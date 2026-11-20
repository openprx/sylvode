/**
 * Hosted frame: message encoding and decoding, origin and source checks, every member of the
 * hosted `SurfaceHost`, width classes at the four slot widths.
 *
 * Run standalone: `bun tests/surface-embed.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import { installRuneModules } from './shell-rune-loader';
import { widthClass } from '../src/lib/surface/width';
import {
	decodeFrameMessage,
	decodeHostMessage,
	envelope,
	type FrameMessage
} from '../src/lib/surface/embed-protocol';
import { embedOrigins, isEmbedOrigin } from '../src/lib/surface/embed-config';
import type { SurfaceTarget } from '../src/lib/surface/context';

await installRuneModules();
const { EmbedSurfaceHost } = await import('../src/lib/surface/embed-host.svelte');

const suite = new Suite('surface-embed');
const ORIGIN = 'http://sv.test';
const PARENT = { name: 'parent' };
const FRAME = 'frame-1';

function setup(overrides: { origins?: readonly string[] } = {}) {
	const posted: FrameMessage[] = [];
	const urls: Array<{ query: Record<string, string | null>; mode: string }> = [];
	const locales: string[] = [];
	const host = new EmbedSurfaceHost({
		parent: PARENT,
		origins: overrides.origins ?? [ORIGIN],
		frameId: FRAME,
		systemTheme: 'light',
		post: (message) => posted.push(message),
		onlocale: (locale) => locales.push(locale),
		writeUrl: (query, mode) => urls.push({ query, mode })
	});
	const send = (data: unknown, source: unknown = PARENT, origin: string = ORIGIN): boolean =>
		host.receive({ data, origin, source });
	return { host, posted, urls, locales, send };
}

/** The payload of a posted message as loose data, for field checks. */
const body = (message: FrameMessage | undefined) => message?.payload as Record<string, unknown>;

const init = (payload: Record<string, unknown> = {}) =>
	envelope(FRAME, 'sv:init', {
		slot: 'panel',
		width: 480,
		theme: 'dark',
		density: 'compact',
		...payload
	});

suite.check('width classes at the slot widths and their boundaries', () => {
	for (const [px, expected] of [
		[0, 'xs'],
		[340, 'xs'],
		[479, 'xs'],
		[480, 'sm'],
		[599, 'sm'],
		[600, 'md'],
		[899, 'md'],
		[900, 'full'],
		[1440, 'full']
	] as const)
		assertEqual(widthClass(px), expected, `${px}`);
});

suite.check('origins: only the frame origin, never the opaque one', () => {
	assertDeepEqual(embedOrigins('http://a.test'), ['http://a.test'], 'self');
	assertDeepEqual(embedOrigins('null'), [], 'opaque');
	assert(isEmbedOrigin('http://a.test', ['http://a.test']), 'allowed');
	assert(!isEmbedOrigin('http://b.test', ['http://a.test']), 'foreign');
	assert(!isEmbedOrigin('null', ['null']), 'null is refused even when listed');
});

suite.check('decodeHostMessage keeps known fields and rejects malformed envelopes', () => {
	const ok = decodeHostMessage({ ...init(), extra: 1, payload: { ...init().payload, junk: true } });
	assert(ok !== null && ok.type === 'sv:init', 'init decoded');
	assertDeepEqual(
		ok.payload,
		{ slot: 'panel', width: 480, theme: 'dark', density: 'compact' },
		'unknown payload fields dropped'
	);
	const withLocale = decodeHostMessage(init({ locale: 'zh' }));
	assertEqual(withLocale?.type === 'sv:init' ? withLocale.payload.locale : null, 'zh', 'locale');
	const badLocale = decodeHostMessage(init({ locale: 'fr' }));
	assertEqual(
		badLocale?.type === 'sv:init' ? badLocale.payload.locale : 'x',
		undefined,
		'bad locale dropped'
	);
	const bad: unknown[] = [
		null,
		'sv:init',
		[],
		{},
		{ ...init(), source: 'other' },
		{ ...init(), protocol: 2 },
		{ ...init(), frameId: '' },
		{ ...init(), frameId: 'a b' },
		{ ...init(), frameId: 'x'.repeat(65) },
		{ ...init(), type: 'sv:unknown' },
		{ ...init(), type: '__proto__' },
		{ ...init(), type: 'constructor' },
		{ ...init(), payload: null },
		{ ...init(), payload: [] },
		init({ slot: 'wide' }),
		init({ width: -1 }),
		init({ width: Number.NaN }),
		init({ width: '480' }),
		init({ theme: 'system' }),
		init({ density: 'tight' }),
		envelope(FRAME, 'sv:resize', { width: Infinity }),
		envelope(FRAME, 'sv:action', { id: '' }),
		envelope(FRAME, 'sv:action', { id: 5 }),
		envelope(FRAME, 'sv:tip-action', {}),
		envelope(FRAME, 'sv:title', { title: 'x', origin: 'y' }) // a frame-to-host type
	];
	for (const [index, data] of bad.entries())
		assertEqual(decodeHostMessage(data), null, `rejected #${index}`);
	assert(
		decodeHostMessage(envelope(FRAME, 'sv:theme', { theme: 'dark', density: 'compact' })),
		'theme'
	);
	assert(decodeHostMessage(envelope(FRAME, 'sv:resize', { width: 340 })), 'resize');
	assert(decodeHostMessage(envelope(FRAME, 'sv:action', { id: 'edit' })), 'action');
	assert(decodeHostMessage(envelope(FRAME, 'sv:tip-action', { id: 't' })), 'tip-action');
});

suite.check('decodeFrameMessage validates every frame-to-host type', () => {
	const target = { kind: 'record', id: 'r', href: '/workspace/w/projects/p/forms/records/r' };
	const good: Array<[string, unknown]> = [
		['sv:ready', { kind: 'record' }],
		['sv:title', { title: 'T', origin: 'Sylvode · T', identifier: 'A-1', context: 'F / T' }],
		['sv:actions', { primary: { id: 'a', label: 'A', kind: 'primary' }, secondary: [] }],
		['sv:tip', { tip: null }],
		[
			'sv:tip',
			{
				tip: {
					id: 't',
					tone: 'danger',
					message: 'm',
					dismissible: false,
					action: { label: 'Retry' }
				}
			}
		],
		['sv:dirty', { dirty: true }],
		['sv:state', { query: { a: '1', b: null }, mode: 'push' }],
		['sv:open', { target, how: 'main' }],
		['sv:close', {}],
		['sv:report', { object: 'o', written: ['a'], unchanged: [] }],
		['sv:height', { px: 320 }]
	];
	for (const [type, payload] of good)
		assert(decodeFrameMessage(envelope(FRAME, type, payload)) !== null, `${type} accepted`);
	const bad: Array<[string, unknown]> = [
		['sv:ready', { kind: '' }],
		['sv:title', { title: 1, origin: 'o' }],
		['sv:title', { title: 'x'.repeat(513), origin: 'o' }],
		['sv:actions', { secondary: [{ id: 'a', label: 'A', kind: 'huge' }] }],
		['sv:actions', { secondary: 'no' }],
		['sv:tip', { tip: { id: 't', tone: 'loud', message: 'm', dismissible: true } }],
		['sv:tip', {}],
		['sv:dirty', { dirty: 'yes' }],
		['sv:state', { query: { a: 5 }, mode: 'push' }],
		['sv:state', { query: {}, mode: 'jump' }],
		['sv:open', { target: { ...target, href: '//evil.test/x' }, how: 'main' }],
		['sv:open', { target: { ...target, href: 'https://evil.test/x' }, how: 'main' }],
		['sv:open', { target: { ...target, href: '/a\\b' }, how: 'main' }],
		['sv:open', { target, how: 'popup' }],
		['sv:report', { object: 'o', written: [1], unchanged: [] }],
		['sv:height', { px: -3 }],
		['sv:init', { slot: 'main', width: 1, theme: 'light', density: 'comfortable' }]
	];
	for (const [index, [type, payload]] of bad.entries())
		assertEqual(
			decodeFrameMessage(envelope(FRAME, type, payload)),
			null,
			`rejected #${index} ${type}`
		);
});

suite.check(
	'receive: foreign origin, foreign source, wrong frame id and junk have no effect',
	() => {
		const { host, send, locales } = setup();
		assertEqual(send(init(), PARENT, 'http://evil.test'), false, 'foreign origin');
		assertEqual(send(init(), PARENT, 'null'), false, 'opaque origin');
		assertEqual(send(init(), { name: 'other' }), false, 'foreign source');
		assertEqual(send(init(), null), false, 'null source');
		assertEqual(send(envelope('someone-else', 'sv:init', init().payload)), false, 'other frame id');
		assertEqual(send('sv:init'), false, 'string');
		assertEqual(
			send({ source: 'sylvode-surface', protocol: 1, frameId: FRAME, type: 'sv:init' }),
			false,
			'no payload'
		);
		assertEqual(send(init({ slot: '<img onerror=x>' })), false, 'bad slot');
		assertEqual(host.initialised, false, 'not initialised');
		assertEqual(host.theme, 'light', 'theme unchanged');
		assertEqual(host.settled, false, 'not settled');
		assertDeepEqual(locales, [], 'locale untouched');
		assertEqual(send(init({ locale: 'zh' })), true, 'valid init');
		assertDeepEqual(locales, ['zh'], 'locale applied');
	}
);

suite.check('a host with no parent accepts nothing', () => {
	const posted: FrameMessage[] = [];
	const host = new EmbedSurfaceHost({
		parent: null,
		origins: [ORIGIN],
		frameId: FRAME,
		systemTheme: 'light',
		post: (m) => posted.push(m)
	});
	assertEqual(
		host.receive({ data: init(), origin: ORIGIN, source: null }),
		false,
		'null source equals null parent'
	);
});

suite.check('init, theme and resize update the reactive state', () => {
	const { host, send } = setup();
	assertEqual(host.frame, 'hosted', 'frame');
	assertEqual(host.globalShortcuts, false, 'no global shortcuts');
	assertEqual(host.slot, 'main', 'default slot');
	assertEqual(host.theme, 'light', 'system theme before init');
	assertEqual(send(init()), true, 'init');
	assertEqual(host.slot, 'panel', 'slot');
	assertEqual(host.width, 480, 'width hint');
	assertEqual(host.widthClass, 'sm', 'class');
	assertEqual(host.theme, 'dark', 'theme');
	assertEqual(host.density, 'compact', 'density');
	assert(host.initialised && host.settled, 'initialised and settled');
	send(envelope(FRAME, 'sv:theme', { theme: 'light', density: 'comfortable' }));
	assertEqual(host.theme, 'light', 'theme changed');
	assertEqual(host.density, 'comfortable', 'density changed');
	send(envelope(FRAME, 'sv:resize', { width: 340 }));
	assertEqual(host.widthClass, 'xs', 'resize reclassifies');
	host.setMeasuredWidth(620);
	assertEqual(host.widthClass, 'md', 'a measured width wins over the hint');
	send(envelope(FRAME, 'sv:resize', { width: 340 }));
	assertEqual(host.width, 620, 'hint does not override a measurement');
});

suite.check('settle: the wait for init ran out, defaults stay', () => {
	const { host } = setup();
	assertEqual(host.settled, false, 'before');
	host.settle();
	assert(host.settled && !host.initialised, 'settled without init');
	assertEqual(host.slot, 'main', 'slot main');
	assertEqual(host.theme, 'light', 'system theme');
});

suite.check('announce posts sv:ready with the frame id, always', () => {
	const { host, posted } = setup();
	host.announce('record');
	host.announce('record');
	assertEqual(posted.length, 2, 'not deduplicated');
	assertDeepEqual(
		posted[0],
		{
			source: 'sylvode-surface',
			protocol: 1,
			frameId: FRAME,
			type: 'sv:ready',
			payload: { kind: 'record' }
		},
		'envelope'
	);
});

suite.check(
	'setTitle: sends title, identifier, context string and origin; skips while loading',
	() => {
		const { host, posted } = setup();
		const surface = host.attach(Symbol('s'));
		surface.setTitle({ title: '', loading: true });
		assertEqual(posted.length, 0, 'nothing while loading');
		surface.setTitle({
			title: 'Northwind order',
			identifier: 'SO-12',
			context: { segments: [{ id: 'f', label: 'Orders' }], current: 'Northwind order' }
		});
		assertEqual(posted.length, 1, 'one message');
		assertDeepEqual(
			posted[0].payload,
			{
				title: 'Northwind order',
				identifier: 'SO-12',
				context: 'Orders / Northwind order',
				origin: 'Sylvode · SO-12'
			},
			'payload'
		);
		surface.setTitle({
			title: 'Northwind order',
			identifier: 'SO-12',
			context: { segments: [{ id: 'f', label: 'Orders' }], current: 'Northwind order' }
		});
		assertEqual(posted.length, 1, 'identical title is not sent again');
		surface.setTitle({ title: 'Plain' });
		assertEqual(body(posted[1]).origin, 'Sylvode · Plain', 'origin falls back to the title');
		surface.setTitle({ title: 'Plain', origin: 'Custom origin' });
		assertEqual(body(posted[2]).origin, 'Custom origin', 'declared origin kept');
	}
);

suite.check('setActions describes actions without functions; sv:action runs them once', () => {
	const { host, posted, send } = setup();
	const surface = host.attach(Symbol('s'));
	const ran: string[] = [];
	surface.setActions({
		primary: { id: 'edit', label: 'Edit', kind: 'primary', run: () => void ran.push('edit') },
		secondary: [
			{ id: 'open', label: 'Open', kind: 'secondary', run: () => void ran.push('open') },
			{
				id: 'del',
				label: 'Delete',
				kind: 'danger-quiet',
				disabledReason: 'No permission',
				run: () => void ran.push('del')
			}
		]
	});
	const message = posted[0];
	assertEqual(message.type, 'sv:actions', 'type');
	assertDeepEqual(
		message.payload,
		{
			primary: { id: 'edit', label: 'Edit', kind: 'primary' },
			secondary: [
				{ id: 'open', label: 'Open', kind: 'secondary' },
				{ id: 'del', label: 'Delete', kind: 'danger-quiet', disabledReason: 'No permission' }
			]
		},
		'descriptors'
	);
	assert(!JSON.stringify(message).includes('run'), 'no function in the message');
	assert(
		decodeFrameMessage(JSON.parse(JSON.stringify(message))),
		'round-trips through the decoder'
	);
	send(envelope(FRAME, 'sv:action', { id: 'open' }));
	send(envelope(FRAME, 'sv:action', { id: 'del' }));
	send(envelope(FRAME, 'sv:action', { id: 'missing' }));
	assertDeepEqual(ran, ['open'], 'only the enabled, known action ran');
	send(envelope(FRAME, 'sv:action', { id: 'edit' }));
	assertDeepEqual(ran, ['open', 'edit'], 'primary runs');
});

suite.check('a throwing or rejecting action does not break the loop', () => {
	const { host, send } = setup();
	const surface = host.attach(Symbol('s'));
	surface.setActions({
		secondary: [
			{
				id: 'boom',
				label: 'B',
				kind: 'secondary',
				run: () => {
					throw new Error('x');
				}
			},
			{ id: 'rej', label: 'R', kind: 'secondary', run: () => Promise.reject(new Error('y')) }
		]
	});
	assertEqual(send(envelope(FRAME, 'sv:action', { id: 'boom' })), true, 'throwing action handled');
	assertEqual(send(envelope(FRAME, 'sv:action', { id: 'rej' })), true, 'rejecting action handled');
});

suite.check('setTip, sv:tip-action, dismissible default', () => {
	const { host, posted, send } = setup();
	const surface = host.attach(Symbol('s'));
	let retried = 0;
	surface.setTip({
		id: 'err',
		tone: 'danger',
		message: 'Could not load',
		action: { label: 'Retry', run: () => void (retried += 1) }
	});
	assertDeepEqual(
		posted[0].payload,
		{
			tip: {
				id: 'err',
				tone: 'danger',
				message: 'Could not load',
				action: { label: 'Retry' },
				dismissible: false
			}
		},
		'danger tip'
	);
	send(envelope(FRAME, 'sv:tip-action', { id: 'other' }));
	assertEqual(retried, 0, 'wrong tip id ignored');
	send(envelope(FRAME, 'sv:tip-action', { id: 'err' }));
	assertEqual(retried, 1, 'tip action ran');
	surface.setTip({ id: 'ok', tone: 'success', message: 'Saved' });
	assertEqual(
		(body(posted[1]).tip as { dismissible: boolean }).dismissible,
		true,
		'success is dismissible by default'
	);
	surface.setTip(null);
	assertDeepEqual(posted[2].payload, { tip: null }, 'cleared');
	send(envelope(FRAME, 'sv:tip-action', { id: 'err' }));
	assertEqual(retried, 1, 'a cleared tip no longer reacts');
});

suite.check('setDirty, syncState, open, close, report, setHeader, setDensity', () => {
	const { host, posted, urls } = setup();
	const surface = host.attach(Symbol('s'));
	surface.setDirty(true);
	assertDeepEqual(posted.at(-1)?.payload, { dirty: true }, 'dirty');
	surface.syncState({ record: 'r1', view: null });
	assertDeepEqual(
		posted.at(-1)?.payload,
		{ query: { record: 'r1', view: null }, mode: 'replace' },
		'state default mode'
	);
	surface.syncState({ record: 'r2' }, 'push');
	assertDeepEqual(
		urls,
		[
			{ query: { record: 'r1', view: null }, mode: 'replace' },
			{ query: { record: 'r2' }, mode: 'push' }
		],
		'frame URL written'
	);
	const target: SurfaceTarget = {
		kind: 'record',
		id: 'r1',
		href: '/workspace/w/projects/p/forms/records/r1',
		workspaceId: 'w',
		label: 'R1'
	};
	surface.open(target, 'replace');
	assertDeepEqual(posted.at(-1)?.payload, { target, how: 'replace' }, 'open');
	surface.open(target, 'replace');
	assertEqual(posted.filter((m) => m.type === 'sv:open').length, 2, 'open is never deduplicated');
	surface.close('/ignored');
	surface.close();
	assertEqual(posted.filter((m) => m.type === 'sv:close').length, 2, 'close is never deduplicated');
	assertDeepEqual(posted.at(-1)?.payload, {}, 'close payload empty');
	surface.report({ object: 'SO-12', written: ['status'], unchanged: ['assignee'] });
	assertDeepEqual(
		posted.at(-1)?.payload,
		{ object: 'SO-12', written: ['status'], unchanged: ['assignee'] },
		'report'
	);
	const before = posted.length;
	surface.setHeader('none');
	surface.setDensity('compact');
	assertEqual(posted.length, before, 'header and density are ignored when hosted');
	assertEqual(host.density, 'comfortable', "density stays the host's");
	for (const message of posted)
		assert(decodeFrameMessage(JSON.parse(JSON.stringify(message))), `${message.type} valid`);
});

suite.check('ownership: a detached or superseded owner is ignored; detach clears', () => {
	const { host, posted, send } = setup();
	const ownerA = Symbol('a');
	const a = host.attach(ownerA);
	const ran: string[] = [];
	a.setActions({
		secondary: [{ id: 'x', label: 'X', kind: 'secondary', run: () => void ran.push('a') }]
	});
	const ownerB = Symbol('b');
	const b = host.attach(ownerB);
	a.setTitle({ title: 'from a' });
	a.close();
	a.open({ kind: 'page', id: 'p', href: '/x' }, 'main');
	a.setDirty(true);
	assertEqual(
		posted.filter((m) => m.type === 'sv:title' || m.type === 'sv:close' || m.type === 'sv:open')
			.length,
		0,
		'superseded owner cannot speak'
	);
	send(envelope(FRAME, 'sv:action', { id: 'x' }));
	assertDeepEqual(ran, [], 'actions of the superseded surface are gone');
	b.setTitle({ title: 'from b' });
	assertEqual(body(posted.at(-1)).title, 'from b', 'new owner speaks');
	host.detach(ownerA);
	assertEqual(host.hasSurface, true, 'detaching an old owner does not drop the new one');
	b.setActions({ secondary: [] });
	const count = posted.length;
	host.detach(ownerB);
	assertEqual(host.hasSurface, false, 'detached');
	assert(posted.length > count, 'detach tells the host actions, tip and dirty were cleared');
	const tail = posted.slice(count).map((m) => m.type);
	assert(tail.includes('sv:tip') && tail.includes('sv:dirty'), 'tip and dirty cleared');
	b.setTitle({ title: 'late' });
	assertEqual(body(posted.at(-1)).title === 'late', false, 'detached owner is ignored');
});

suite.check('requestClose and reportHeight', () => {
	const { host, posted, send } = setup();
	host.requestClose();
	assertEqual(posted.at(-1)?.type, 'sv:close', 'Escape becomes a close request');
	host.reportHeight(300.2);
	assertEqual(
		posted.filter((m) => m.type === 'sv:height').length,
		0,
		'height only for the inline slot'
	);
	send(init({ slot: 'inline' }));
	host.reportHeight(300.2);
	assertDeepEqual(posted.at(-1)?.payload, { px: 301 }, 'rounded up');
	host.reportHeight(300.2);
	assertEqual(
		posted.filter((m) => m.type === 'sv:height').length,
		1,
		'unchanged height not repeated'
	);
});

suite.check('ambient: hosted, no global shortcuts, open and close go to the host', () => {
	const { host, posted, send } = setup();
	send(init());
	const ambient = host.ambient();
	assertEqual(ambient.frame, 'hosted', 'frame');
	assertEqual(ambient.globalShortcuts, false, 'shortcuts');
	assertEqual(ambient.theme, 'dark', 'theme');
	assertEqual(ambient.density, 'compact', 'density');
	ambient.open({ kind: 'page', id: 'p', href: '/p' }, 'main');
	ambient.close();
	assertDeepEqual(
		posted.map((m) => m.type),
		['sv:open', 'sv:close'],
		'messages'
	);
});

finish(suite.result());
