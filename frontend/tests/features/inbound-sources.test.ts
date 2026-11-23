/**
 * The inbound sources section (S1, S2, S3): the form rules and request bodies, failure routing
 * (field message, 409, other), the delete impact, paging, and the rendered states of the
 * components: credentials never rendered after the finish, a hostile event shown as text, the
 * delete dialog listing the bound rules, the suspended card.
 *
 * Run standalone: `bun test tests/features/inbound-sources.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual } from '../support/harness';
import { installSvelteSsr, renderToHtml } from '../support/svelte-ssr';
import {
	ALLOWLIST_MAX,
	changedFields,
	createBody,
	defaultDraft,
	draftOf,
	hasChanges,
	isValid,
	parseAllowlist,
	routeFailure,
	serverField,
	turnsSignatureOff,
	validateDraft,
	wholeNumber
} from '../../src/lib/project/inbound/source-form';
import {
	appendEvents,
	boundRules,
	hasOlder,
	olderCursor,
	reportedJson,
	replaceSource,
	ruleNameOf,
	ruleNames,
	withoutSource
} from '../../src/lib/project/inbound/source-view';
import {
	NO_SECRETS,
	discard,
	finish as finishSecrets,
	setStored,
	showSecrets
} from '../../src/lib/project/inbound/secrets-state';
import { parseEvent, parseRule, parseSource } from '../../src/lib/project/inbound/wire';
import type { InboundEvent, Rule, Source } from '../../src/lib/project/inbound/wire';

await installSvelteSsr();
const { default: SourceSecrets } = await import('../../src/lib/project/inbound/SourceSecrets.svelte');
const { default: EventDetailDialog } =
	await import('../../src/lib/project/inbound/EventDetailDialog.svelte');
const { default: SourceDialogs } = await import('../../src/lib/project/inbound/SourceDialogs.svelte');
const { default: SourceCard } = await import('../../src/lib/project/inbound/SourceCard.svelte');

const suite = new Suite('inbound-sources');

const sourceJson = {
	id: 's1',
	workspace_id: 'w1',
	project_id: 'p1',
	name: 'prx-waf edge-01',
	token_prefix: 'svi_1a2b3c4d',
	require_signature: true,
	has_signing_secret: true,
	ip_allowlist: ['203.0.113.0/24'],
	acting_user_id: 'u1',
	enabled: true,
	rate_per_minute: 120,
	daily_cap: 20000,
	last_used_at: null,
	last_used_ip: null,
	created_at: '2026-11-21T10:00:00Z',
	updated_at: '2026-11-21T10:00:00Z'
};
const source = parseSource(sourceJson) as Source;
assert(source !== null, 'the fixture source parses');

function ruleJson(id: string, name: string, sourceId: string | null) {
	return {
		id,
		workspace_id: 'w1',
		project_id: 'p1',
		name,
		position: 1,
		enabled: true,
		source_id: sourceId,
		kind_patterns: [],
		min_severity: 'high',
		action: 'work_item',
		bot_user_id: null,
		instructions: '',
		priority_map: { critical: 'urgent', high: 'high', medium: 'medium', low: 'low', info: 'low' },
		label_ids: [],
		notify_user_ids: [],
		max_tasks_per_hour: 6,
		max_active_tasks: 2,
		dedup_window_secs: 3600,
		rearm_after_secs: 900,
		task_max_attempts: 3,
		version: 1
	};
}
const rule = (id: string, name: string, sourceId: string | null): Rule =>
	parseRule(ruleJson(id, name, sourceId)) as Rule;

const HOSTILE = '<img src=x onerror=alert(1)>';
function eventOf(over: Record<string, unknown> = {}): InboundEvent {
	return parseEvent({
		id: 'e1',
		event_id: 'evt-0001',
		kind: 'waf.attack_detected',
		severity_rank: 3,
		dedup_key: 'k',
		occurred_at: '2026-11-21T10:00:00Z',
		received_at: '2026-11-21T10:00:01Z',
		title: HOSTILE,
		summary: '[click](javascript:alert(1)) <script>alert(2)</script>',
		subject: { host: HOSTILE },
		payload: { note: '[link](https://evil.example)', html: '<b>bold</b>' },
		signed: true,
		client_ip: '203.0.113.7',
		outcome: 'alert_opened',
		rule_id: 'r1',
		alert_id: 'a1',
		task_id: null,
		work_item: { id: 'i1', identifier: 'OPS-12' },
		...over
	}) as InboundEvent;
}

// --- form rules -----------------------------------------------------------------------------

suite.check('the create form starts on the documented defaults', () => {
	const draft = defaultDraft();
	assertEqual(draft.requireSignature, true, 'signature required');
	assertEqual(draft.rate, '120', 'rate');
	assertEqual(draft.cap, '20000', 'cap');
	assertEqual(draft.allowlist, '', 'any address');
	assertEqual(isValid(validateDraft({ ...draft, name: 'x' })), true, 'a named default is valid');
});

suite.check('name rules: trimmed 1 to 100 characters', () => {
	const base = defaultDraft();
	assertEqual(validateDraft({ ...base, name: '   ' }).name, 'inbound.error.nameRequired', 'blank');
	assertEqual(validateDraft({ ...base, name: 'a'.repeat(100) }).name, undefined, '100 is fine');
	assertEqual(
		validateDraft({ ...base, name: 'a'.repeat(101) }).name,
		'inbound.error.nameLength',
		'101'
	);
	assertEqual(
		validateDraft({ ...base, name: ` ${'a'.repeat(100)} ` }).name,
		undefined,
		'trimmed before counting'
	);
});

suite.check('rate and cap ranges and the number reader', () => {
	const base = { ...defaultDraft(), name: 'x' };
	for (const bad of ['0', '6001', '', '1.5', '-1', '1e3', 'abc'])
		assertEqual(
			validateDraft({ ...base, rate: bad }).rate,
			'inbound.error.rateRange',
			`rate ${bad}`
		);
	for (const ok of ['1', '6000', ' 90 '])
		assertEqual(validateDraft({ ...base, rate: ok }).rate, undefined, `rate ${ok}`);
	assertEqual(validateDraft({ ...base, cap: '0' }).cap, 'inbound.error.capRange', 'cap 0');
	assertEqual(validateDraft({ ...base, cap: '1000001' }).cap, 'inbound.error.capRange', 'cap over');
	assertEqual(validateDraft({ ...base, cap: '1000000' }).cap, undefined, 'cap max');
	assertEqual(wholeNumber('42'), 42, 'whole');
	assertEqual(wholeNumber('4.2'), null, 'fraction');
});

suite.check('allow-list: lines trimmed, blanks dropped, at most 64', () => {
	assertDeepEqual(
		parseAllowlist(' 203.0.113.7 \r\n\n10.0.0.0/8\n  \n'),
		['203.0.113.7', '10.0.0.0/8'],
		'parsed'
	);
	const base = { ...defaultDraft(), name: 'x' };
	const lines = (n: number) => Array.from({ length: n }, (_, i) => `10.0.${i}.1`).join('\n');
	assertEqual(
		validateDraft({ ...base, allowlist: lines(ALLOWLIST_MAX) }).allowlist,
		undefined,
		'64'
	);
	assertEqual(
		validateDraft({ ...base, allowlist: lines(ALLOWLIST_MAX + 1) }).allowlist,
		'inbound.error.allowlistCount',
		'65'
	);
});

suite.check('create states every field and never sends a format', () => {
	const body = createBody({ ...defaultDraft(), name: '  prx-waf edge-01 ', allowlist: '10.0.0.1' });
	assertDeepEqual(
		body,
		{
			name: 'prx-waf edge-01',
			require_signature: true,
			rate_per_minute: 120,
			daily_cap: 20000,
			ip_allowlist: ['10.0.0.1']
		},
		'full create body'
	);
	assertEqual('format' in body, false, 'no format');
});

suite.check('edit sends only the changed fields; no change means nothing to save', () => {
	const same = draftOf(source);
	assertEqual(hasChanges(changedFields(same, source)), false, 'untouched');
	assertDeepEqual(changedFields({ ...same, name: ' New ' }, source), { name: 'New' }, 'name only');
	assertDeepEqual(
		changedFields({ ...same, rate: '60', cap: '5000' }, source),
		{ rate_per_minute: 60, daily_cap: 5000 },
		'numbers'
	);
	assertDeepEqual(
		changedFields({ ...same, allowlist: '' }, source),
		{ ip_allowlist: [] },
		'an emptied allow-list clears with []'
	);
	assertEqual(
		hasChanges(changedFields({ ...same, allowlist: ' 203.0.113.0/24 \n' }, source)),
		false,
		'whitespace is not a change'
	);
});

suite.check('turning the signature off is the change that asks first', () => {
	const same = draftOf(source);
	assertEqual(
		turnsSignatureOff(changedFields({ ...same, requireSignature: false }, source)),
		true,
		'off'
	);
	assertEqual(turnsSignatureOff(changedFields({ ...same, name: 'x' }, source)), false, 'other');
	const unsigned = { ...source, require_signature: false };
	assertEqual(
		turnsSignatureOff(changedFields({ ...draftOf(unsigned), requireSignature: true }, unsigned)),
		false,
		'turning it on does not ask'
	);
});

// --- failure routing ------------------------------------------------------------------------

suite.check('a server message goes to the field it names', () => {
	assertEqual(serverField('name must be 1 to 100 characters'), 'name', 'name');
	assertEqual(
		serverField('ip_allowlist entry 192.0.2.5/24 has host bits set; use 192.0.2.0/24'),
		'allowlist',
		'allowlist'
	);
	assertEqual(serverField('rate_per_minute must be 1 to 6000'), 'rate', 'rate');
	assertEqual(serverField('daily_cap must be 1 to 1000000'), 'cap', 'cap');
	assertEqual(serverField('namespace broken'), null, 'a word that merely starts with name');
	assertEqual(serverField('something else'), null, 'other');
});

suite.check('failures: field, 409 conflict, other', () => {
	assertEqual(
		routeFailure({ code: 400, message: 'ip_allowlist entry 1.2.3.4/0 is refused' }).type,
		'field',
		'field'
	);
	assertEqual(routeFailure({ code: 409, message: 'source changed' }).type, 'stale', '409');
	assertEqual(routeFailure({ code: 500, message: 'boom' }).type, 'other', '500');
	assertEqual(
		routeFailure({ code: 409, message: 'name must be unique' }).type,
		'field',
		'a named field wins over the status'
	);
});

// --- view helpers ---------------------------------------------------------------------------

suite.check('delete impact: the rules bound to the source, by name', () => {
	const rules = [
		rule('r1', 'WAF high to triage', 's1'),
		rule('r2', 'Any source', null),
		rule('r3', 'Edge noise', 's1'),
		rule('r4', 'Other', 's2')
	];
	const bound = boundRules(rules, 's1');
	assertDeepEqual(
		bound.map((r) => r.id),
		['r1', 'r3'],
		'bound only'
	);
	assertEqual(ruleNames(bound), 'WAF high to triage, Edge noise', 'names');
	assertEqual(boundRules(rules, 'none').length, 0, 'none bound');
});

suite.check('event paging: older cursor, merge without duplicates, full page', () => {
	const a = eventOf({ id: 'a', received_at: '2026-11-21T10:00:03Z' });
	const b = eventOf({ id: 'b', received_at: '2026-11-21T10:00:02Z' });
	assertEqual(olderCursor([a, b]), '2026-11-21T10:00:02Z', 'last row');
	assertEqual(olderCursor([]), null, 'empty');
	assertDeepEqual(
		appendEvents([a, b], [b, eventOf({ id: 'c' })]).map((e) => e.id),
		['a', 'b', 'c'],
		'dedupe'
	);
	assertEqual(hasOlder(50), true, 'full page');
	assertEqual(hasOlder(49), false, 'short page');
});

suite.check('reported values: pretty JSON, null as empty, rule name lookup, list updates', () => {
	assertEqual(reportedJson({ a: 1 }), '{\n  "a": 1\n}', 'two spaces');
	assertEqual(reportedJson(null), '', 'null');
	assertEqual(ruleNameOf([rule('r1', 'N', null)], 'r1'), 'N', 'found');
	assertEqual(ruleNameOf([rule('r1', 'N', null)], 'gone'), null, 'deleted rule');
	assertEqual(ruleNameOf(null, 'r1'), null, 'unknown rules');
	const next = { ...source, name: 'renamed' };
	assertEqual(replaceSource([source], next)[0].name, 'renamed', 'replace');
	assertEqual(withoutSource([source], 's1').length, 0, 'remove');
});

// --- rendered states ------------------------------------------------------------------------

const TOKEN = `svi_${'a1'.repeat(32)}`;
const SECRET = `svs_${'b2'.repeat(32)}`;
const ENDPOINT = 'https://sv.example/api/v1/inbound/events';
const shownState = () => showSecrets({ endpoint: ENDPOINT, token: TOKEN, signingSecret: SECRET });
const secretProps = (secrets: ReturnType<typeof shownState>) => ({
	secrets,
	sourceName: 'prx-waf edge-01',
	requireSignature: true,
	onstored: () => {},
	ondone: () => {}
});

await suite.checkAsync(
	'credentials: shown with the warning, gone after Done and after leaving',
	async () => {
		const html = await renderToHtml(SourceSecrets, secretProps(shownState()));
		for (const text of [ENDPOINT, TOKEN, SECRET])
			assert(html.includes(text), 'a value is on screen while shown');
		assert(html.includes('inbound.secrets.title'), 'the warning');
		assert(html.includes('inbound.secrets.doneReason'), 'Done says why it is disabled');
		assert(html.includes('inbound.secrets.copyAll'), 'copy all as JSON');

		const stored = setStored(shownState(), true);
		const done = finishSecrets(stored);
		const afterDone = await renderToHtml(SourceSecrets, secretProps(done));
		const afterLeave = await renderToHtml(SourceSecrets, secretProps(discard()));
		const afterNone = await renderToHtml(SourceSecrets, secretProps(NO_SECRETS));
		for (const page of [afterDone, afterLeave, afterNone])
			for (const text of [ENDPOINT, TOKEN, SECRET, 'svi_', 'svs_'])
				assertEqual(
					page.includes(text),
					false,
					`a finished state never renders ${text.slice(0, 4)}`
				);
	}
);

await suite.checkAsync('credentials: Done is not offered before the box is checked', async () => {
	const before = await renderToHtml(SourceSecrets, secretProps(shownState()));
	assert(before.includes('inbound.secrets.doneReason'), 'reason while unconfirmed');
	const after = await renderToHtml(SourceSecrets, secretProps(setStored(shownState(), true)));
	assertEqual(after.includes('inbound.secrets.doneReason'), false, 'no reason once confirmed');
});

await suite.checkAsync('a hostile event renders as text only', async () => {
	const html = await renderToHtml(EventDetailDialog, {
		open: true,
		event: eventOf(),
		sourceId: 's1',
		workspaceId: 'w1',
		projectId: 'p1',
		rules: [rule('r1', 'WAF high to triage', 's1')],
		onclose: () => {}
	});
	assertEqual(html.includes('<img'), false, 'no element made from the title');
	assertEqual(html.includes('<script'), false, 'no script element');
	assertEqual(html.includes('<b>'), false, 'no element made from the payload');
	assert(html.includes('&lt;img src=x onerror=alert(1)>'), 'the title is escaped text');
	assert(html.includes('&lt;b>bold&lt;/b>'), 'the payload is escaped text');
	assert(html.includes('[link]'), 'the Markdown stays literal');
	const reported = html.slice(html.indexOf('event-reported'));
	assertEqual(reported.includes('<a '), false, 'the reported fields make no links');
	assert(html.includes('inbound.events.untrusted'), 'the untrusted notice');
	assert(html.includes('WAF high to triage'), 'the rule name');
	assert(html.includes('OPS-12'), 'the work item link');
	assert(html.includes('/workspace/w1/projects/p1/issues/i1'), 'the link target');
});

await suite.checkAsync(
	'event detail: a rule that is gone and a work item that is not there',
	async () => {
		const html = await renderToHtml(EventDetailDialog, {
			open: true,
			event: eventOf({ rule_id: 'gone', work_item: null, outcome: 'future_code' }),
			sourceId: 's1',
			workspaceId: 'w1',
			projectId: 'p1',
			rules: [],
			onclose: () => {}
		});
		assert(html.includes('inbound.events.detail.deletedRule'), 'deleted rule wording');
		assert(html.includes('future_code'), 'an unknown outcome prints its code');
		assertEqual(html.includes('/issues/'), false, 'no link without a work item');
	}
);

await suite.checkAsync('event detail renders nothing while closed', async () => {
	const html = await renderToHtml(EventDetailDialog, {
		open: false,
		event: null,
		sourceId: 's1',
		workspaceId: 'w1',
		projectId: 'p1',
		rules: null,
		onclose: () => {}
	});
	assertEqual(html.includes(HOSTILE), false, 'nothing');
});

await suite.checkAsync(
	'the delete dialog names the bound rules; none means no sentence',
	async () => {
		const base = { source, busy: false, onconfirm: () => {}, onclose: () => {} };
		const bound = [rule('r1', 'WAF high to triage', 's1'), rule('r3', 'Edge noise', 's1')];
		const html = await renderToHtml(SourceDialogs, { ...base, pending: 'delete', bound });
		assert(html.includes('inbound.sources.deleteDialog.title'), 'the delete dialog is open');
		assert(html.includes('delete-source-rules'), 'the rule sentence');
		assert(html.includes('inbound.sources.deleteDialog.bodyRules'), 'its message');
		const none = await renderToHtml(SourceDialogs, { ...base, pending: 'delete', bound: [] });
		assertEqual(none.includes('delete-source-rules'), false, 'omitted when no rule is bound');
		const closed = await renderToHtml(SourceDialogs, { ...base, pending: null, bound });
		assertEqual(closed.includes('inbound.sources.deleteDialog.title'), false, 'closed');
		for (const [pending, title] of [
			['pause', 'pauseDialog'],
			['rotate', 'rotateDialog'],
			['takeOver', 'takeOverDialog']
		] as const) {
			const dialog = await renderToHtml(SourceDialogs, { ...base, pending, bound: [] });
			assert(dialog.includes(`inbound.sources.${title}.title`), `${pending} dialog`);
		}
	}
);

await suite.checkAsync(
	'cards: suspended wins and offers take over; waiting shows the lost hint',
	async () => {
		const props = {
			projectId: 'p1',
			members: null,
			now: new Date('2026-11-21T12:00:00Z'),
			editing: false,
			eventsOpen: false,
			onedit: () => {},
			oncanceledit: () => {},
			onsaved: () => {},
			onevents: () => {},
			onrotated: () => {},
			ondeleted: () => {},
			onstale: () => {}
		};
		const suspended = await renderToHtml(SourceCard, {
			...props,
			source: { ...source, suspended: true, acting_user_name: 'Ana' }
		});
		assert(suspended.includes('inbound.sources.status.suspended'), 'suspended chip');
		assert(suspended.includes('inbound.sources.suspendedBody'), 'tip bar');
		assert(suspended.includes('inbound.sources.takeOver'), 'take over');
		const waiting = await renderToHtml(SourceCard, {
			...props,
			source: { ...source, suspended: false }
		});
		assert(waiting.includes('inbound.sources.status.waiting'), 'waiting chip');
		assert(waiting.includes('source-lost-hint'), 'lost credentials hint');
		assertEqual(
			waiting.includes('inbound.sources.takeOver'),
			false,
			'no take over for a working source'
		);
		assert(waiting.includes('svi_1a2b3c4d…'), 'only the token prefix');
		assert(waiting.includes('inbound.sources.pause'), 'pause offered');
		const paused = await renderToHtml(SourceCard, {
			...props,
			source: { ...source, suspended: false, enabled: false }
		});
		assert(paused.includes('inbound.sources.status.paused'), 'paused chip');
		assert(paused.includes('inbound.sources.resume'), 'resume offered');
	}
);

// --- sources of the section never keep or log the credentials -------------------------------

function files(dir: string): Array<{ file: string; text: string }> {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return files(path);
		return /\.(ts|svelte)$/.test(entry.name)
			? [{ file: path, text: readFileSync(path, 'utf8') }]
			: [];
	});
}

suite.check('the sources code never stores, logs or renders untrusted text as markup', () => {
	const mine = [
		'SourcesSection',
		'SourceCard',
		'SourceForm',
		'SourceSecrets',
		'SourceEvents',
		'EventDetailDialog',
		'SourceDialogs'
	].map((name) => ({
		file: name,
		text: readFileSync(`src/lib/project/inbound/${name}.svelte`, 'utf8')
	}));
	const all = [
		...mine,
		...files('src/lib/project/inbound').filter((f) => /source-(form|view)\.ts$/.test(f.file)),
		{ file: 'inbound.ts', text: readFileSync('src/lib/api/inbound.ts', 'utf8') }
	];
	assert(all.length >= 10, 'the sources were found');
	const forbidden: Array<[RegExp, string]> = [
		[/localStorage|sessionStorage|indexedDB/, 'browser storage'],
		[/console\.(log|debug|info|warn|error)/, 'logging'],
		[/\{@html/, 'markup injection'],
		[/innerHTML|insertAdjacentHTML/, 'markup injection'],
		[/from ['"]svelte\/store['"]|writable\(|\$app\/stores/, 'a store'],
		[/\$page\b/, 'the page store'],
		[/renderMarkdown|marked\(|markdown/i, 'the Markdown renderer']
	];
	for (const { file, text } of all)
		for (const [pattern, what] of forbidden)
			assert(!pattern.test(text), `${file} must not use ${what}`);
	// Only the section (which hands the answer over) and the one-time screen touch the credentials.
	for (const { file, text } of mine) {
		if (file === 'SourcesSection' || file === 'SourceSecrets') continue;
		assert(
			!/signing_secret|signingSecret|answer\.token|values\.token/.test(text),
			`${file} does not read the credentials`
		);
	}
});

test(suite.name, () => {
	const result = suite.result();
	nodeAssert.equal(result.failed, 0, result.failures.join('\n'));
});
