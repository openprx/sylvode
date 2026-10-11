/**
 * Presence (`collab-protocol-v1.md` `presence` frame + capability; `limits-v1.md` presence rows;
 * `versions/v0.5-collaboration.md` "显示其他用户 cursor/selection 和 block presence，但不把 presence
 * 当锁").
 *
 * - `hello` declares the `presence` capability;
 * - outgoing presence is throttled to at most 4 frames/s and carries the v1 payload, a TTL within
 *   `1..30` and the document id;
 * - a payload over `presence_payload_bytes_max` (8 KiB) is refused locally and never sent;
 * - received presence expires locally after its TTL (30 s when omitted), a refresh of the same
 *   session replaces the entry instead of adding one, and a connection loss clears the map;
 * - presence never touches the engine doc: no local update, no outbox entry, no version change;
 * - two clients on the same document see each other through the server fan-out, and a peer's
 *   presence does not block an edit.
 *
 * Run standalone: `bun tests/flow-presence.test.ts`
 */

import { Suite, assert, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	MockWebSocket,
	OBJECT_ID,
	WORKSPACE_ID,
	edit,
	freshIndexedDb,
	installFakes,
	objectView,
	tick,
	OUTBOX_DB
} from './support/collab-fakes';
import {
	PRESENCE_MIN_INTERVAL_MS,
	PresenceThrottle,
	RemotePresenceMap,
	buildPresencePayload,
	parsePresencePayload,
	presenceColor,
	presencePayloadBytes
} from '../src/lib/flow/presence';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';

const suite = new Suite('flow_presence');

const ALICE = { userId: 'u-alice', name: 'Alice', color: '#2563eb' };
const BOB = { userId: 'u-bob', name: 'Bob', color: '#db2777' };

class FakeClock {
	t = 1_000_000;
	now(): number {
		return this.t;
	}
}

// ---- pure parts ---------------------------------------------------------------------------------

suite.check('the throttle sends at most one frame per 250 ms, the latest value last', () => {
	const clock = new FakeClock();
	const sent: number[] = [];
	const throttle = new PresenceThrottle<number>(
		(v) => sent.push(v),
		PRESENCE_MIN_INTERVAL_MS,
		clock
	);
	for (let i = 0; i < 40; i += 1) {
		throttle.push(i);
		clock.t += 25; // 40 changes over one second
		if (clock.t % 250 === 0) throttle.flush();
	}
	throttle.flush();
	throttle.cancel();
	assert(
		sent.length <= 5,
		`sent ${sent.length} frames for one second of changes (limit 4/s + trailing)`
	);
	assertEqual(sent[0], 0, 'the first change goes out at once');
	assertEqual(sent.at(-1), 39, 'the trailing send carries the latest value');
});

suite.check('payload size is measured as UTF-8 JSON bytes', () => {
	const payload = buildPresencePayload({ ...ALICE, name: '汉'.repeat(10) }, null);
	assertEqual(
		presencePayloadBytes(payload),
		new TextEncoder().encode(JSON.stringify(payload)).length,
		'byte length, not string length'
	);
	assert(presencePayloadBytes(payload) > JSON.stringify(payload).length, 'CJK counts as 3 bytes');
});

suite.check('a malformed peer payload is dropped, not half-rendered', () => {
	assertEqual(parsePresencePayload({ v: 2, user: ALICE, cursor: null }), null, 'unknown version');
	assertEqual(
		parsePresencePayload({ v: 1, user: { userId: 'x' }, cursor: null }),
		null,
		'bad user'
	);
	assertEqual(
		parsePresencePayload({ v: 1, user: ALICE, cursor: { blockId: 'b', anchor: -1, head: 0 } }),
		null,
		'bad cursor'
	);
	assert(
		parsePresencePayload({ v: 1, user: ALICE, cursor: { blockId: 'b', anchor: 1, head: 2 } }),
		'valid'
	);
});

suite.check('TTL: received presence expires locally; a refresh replaces, never adds', () => {
	const clock = new FakeClock();
	const map = new RemotePresenceMap(clock);
	map.receive('s1', buildPresencePayload(ALICE, null), undefined);
	map.receive('s2', buildPresencePayload(BOB, null), 5);
	assertEqual(map.size, 2, 'two peers');
	clock.t += 4_999;
	assertEqual(map.size, 2, 'both alive before 5 s');
	clock.t += 2;
	assertEqual(map.size, 1, 'the 5 s entry expired');
	map.receive('s1', buildPresencePayload(ALICE, { blockId: 'b', anchor: 1, head: 1 }), 30);
	assertEqual(map.size, 1, 'same session refreshes in place');
	clock.t += 29_999;
	assertEqual(map.size, 1, 'refreshed entry still alive');
	clock.t += 2;
	assertEqual(map.size, 0, 'expired 30 s after the last refresh (default TTL)');
	map.receive('s3', buildPresencePayload(ALICE, null), 999);
	clock.t += 30_001;
	assertEqual(map.size, 0, 'a TTL above 30 is clamped to 30');
});

const INJECTED_COLOR = 'red;position:fixed;inset:0;z-index:99999;width:100vw;height:100vh';

suite.check('a peer colour that is not #rrggbb is replaced by the palette colour', () => {
	const clock = new FakeClock();
	const map = new RemotePresenceMap(clock);
	for (const [i, color] of [
		INJECTED_COLOR,
		'url(https://tracker.example/x.png)',
		'#2563eb;background-image:url(x)',
		'#12345',
		'rgb(0,0,0)',
		''
	].entries()) {
		assert(
			map.receive(`s${i}`, { v: 1, user: { userId: 'u-x', name: 'X', color }, cursor: null }, 30, {
				user_id: 'actor-1'
			}),
			`a bad colour does not hide the peer (${color})`
		);
		const peer = map.list().find((p) => p.sessionId === `s${i}`);
		assertEqual(peer?.user.color, presenceColor('actor-1'), `palette colour for ${color}`);
	}
	map.receive(
		'ok',
		{ v: 1, user: { userId: 'u-x', name: 'X', color: '#AbCdEf' }, cursor: null },
		30,
		{
			user_id: 'actor-1'
		}
	);
	assertEqual(
		map.list().find((p) => p.sessionId === 'ok')?.user.color,
		'#AbCdEf',
		'a #rrggbb colour is kept'
	);
});

suite.check('a peer name is plain text of at most 64 characters', () => {
	const map = new RemotePresenceMap(new FakeClock());
	map.receive(
		's1',
		{
			v: 1,
			user: {
				userId: 'u',
				name: '<img src=x onerror=alert(1)>Bob\u0000\u202e\u001b[31m',
				color: '#000000'
			},
			cursor: null
		},
		30,
		{ user_id: 'a' }
	);
	const name = map.list()[0]?.user.name ?? '';
	assert(!/[<>]/.test(name), `no markup survives: ${JSON.stringify(name)}`);
	const isControl = (ch: string) => {
		const code = ch.codePointAt(0) ?? 0;
		return (
			code < 0x20 ||
			(code >= 0x7f && code <= 0x9f) ||
			(code >= 0x202a && code <= 0x202e) ||
			(code >= 0x2066 && code <= 0x2069)
		);
	};
	assert(!Array.from(name).some(isControl), `no control characters: ${JSON.stringify(name)}`);
	assert(name.includes('Bob'), 'the text itself is kept');
	map.receive(
		's2',
		{ v: 1, user: { userId: 'u', name: '名'.repeat(100), color: '#000000' }, cursor: null },
		30,
		{
			user_id: 'a'
		}
	);
	assertEqual(
		Array.from(map.list().find((p) => p.sessionId === 's2')?.user.name ?? '').length,
		64,
		'truncated to 64 characters'
	);
	assertEqual(
		map.receive(
			's3',
			{ v: 1, user: { userId: 'u', name: '<b></b>\u0007', color: '#000000' }, cursor: null },
			30,
			{
				user_id: 'a'
			}
		),
		false,
		'a name that is nothing but markup and control characters drops the presence'
	);
});

suite.check("identity is the server's actor, never the payload's claimed userId", () => {
	const map = new RemotePresenceMap(new FakeClock());
	map.receive(
		's1',
		{ v: 1, user: { userId: 'u-owner', name: 'Owner', color: '#000000' }, cursor: null },
		30,
		{
			user_id: 'u-member'
		}
	);
	const attested = map.list()[0];
	assertEqual(attested.user.userId, 'u-member', 'the actor names the peer');
	assertEqual(attested.attested, true, 'attested by the server');
	map.receive(
		's2',
		{ v: 1, user: { userId: 'u-owner', name: 'Owner', color: 'red' }, cursor: null },
		30,
		undefined
	);
	const legacy = map.list().find((p) => p.sessionId === 's2');
	assertEqual(legacy?.attested, false, 'a frame without actor is not attested');
	assertEqual(legacy?.user.color, presenceColor('u-owner'), 'the claimed id only seeds the colour');
});

suite.check('the canvas and the editor never put a peer value into a style unchecked', () => {
	const root = join(new URL('..', import.meta.url).pathname, 'src', 'lib');
	const canvas = readFileSync(join(root, 'components', 'flow', 'FlowCanvas.svelte'), 'utf8');
	const adapter = readFileSync(join(root, 'flow', 'editor-adapter.ts'), 'utf8');
	for (const [label, source] of [
		['FlowCanvas.svelte', canvas],
		['editor-adapter.ts', adapter]
	] as const) {
		assert(!/\$\{peer\.user\.color\}/.test(source), `${label} interpolates peer.user.color`);
		assert(!/=\s*peer\.user\.color\b/.test(source), `${label} assigns peer.user.color`);
		assert(!/setProperty\([^)]*peer\.user\.color\)/.test(source), `${label} sets peer.user.color`);
		assert(
			/safePresenceColor\(/.test(source),
			`${label} renders colours through safePresenceColor`
		);
	}
});

// ---- through the real session ------------------------------------------------------------------

async function openClient(server: FakeCollabServer, idb?: IDBFactory) {
	const factory = idb ?? (await freshIndexedDb());
	const store = await IndexedDbOutboxStore.open(factory, OUTBOX_DB);
	assert(store, 'fake IndexedDB opens');
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity: new FakeConnectivity(), answerTimeoutMs: 1500 }
	});
	const entry = await repository.open({
		workspaceId: WORKSPACE_ID,
		objectId: OBJECT_ID,
		signal: new AbortController().signal,
		object: objectView() as unknown as FlowObjectView
	});
	const socket = MockWebSocket.instances[MockWebSocket.instances.length - 1];
	return { repository, entry, store, socket, factory };
}

const server = new FakeCollabServer();
const restore = await installFakes(server);

await suite.checkAsync('hello declares the presence capability', async () => {
	const a = await openClient(server);
	try {
		const hello = a.socket.sent.find((f) => f.type === 'hello');
		assert(Array.isArray(hello?.capabilities), 'capabilities sent');
		assert((hello?.capabilities as string[]).includes('presence'), 'presence capability');
	} finally {
		await a.repository.close(OBJECT_ID);
	}
});

await suite.checkAsync('outgoing presence: frame shape, throttle, payload pre-check', async () => {
	const a = await openClient(server);
	try {
		a.entry.session.setPresenceIdentity(ALICE);
		for (let i = 0; i < 20; i += 1) {
			a.entry.session.setPresence({ blockId: 'cid:1@1:Map', anchor: i, head: i });
		}
		await tick(PRESENCE_MIN_INTERVAL_MS + 60);
		const frames = a.socket.sent.filter((f) => f.type === 'presence');
		assert(
			frames.length >= 1 && frames.length <= 2,
			`20 changes in one burst sent ${frames.length} frames`
		);
		const last = frames.at(-1) as Record<string, unknown>;
		assertEqual(last.document_id, '22222222-2222-4222-8222-222222222222', 'document id');
		assertEqual(last.ttl_seconds, 30, 'ttl within 1..30');
		const payload = parsePresencePayload(last.payload);
		assertEqual(payload?.cursor?.anchor, 19, 'the latest cursor wins');
		assertEqual(payload?.user.userId, ALICE.userId, 'identity summary');

		const before = a.entry.session.presenceFrameCount;
		a.entry.session.setPresenceIdentity({ ...ALICE, name: 'x'.repeat(9000) });
		a.entry.session.setPresence({ blockId: 'cid:1@1:Map', anchor: 0, head: 0 });
		await tick(PRESENCE_MIN_INTERVAL_MS + 60);
		assertEqual(a.entry.session.presenceFrameCount, before, 'an oversized payload is never sent');
		const refusal = a.entry.session.presenceRefusal;
		assertEqual(refusal?.limitKind, 'presence_payload_bytes', 'refused with the limit kind');
		assertEqual(refusal?.limit, 8192, 'against the 8 KiB ceiling');
		assert((refusal?.observed ?? 0) > 8192, 'observed size reported');
	} finally {
		await a.repository.close(OBJECT_ID);
	}
});

await suite.checkAsync(
	'two clients see each other; presence never enters the engine doc',
	async () => {
		server.connectionUserIds = ['attested-alice', 'attested-bob'];
		const a = await openClient(server);
		const b = await openClient(server);
		try {
			a.entry.session.setPresenceIdentity(ALICE);
			b.entry.session.setPresenceIdentity(BOB);
			const aVersion = JSON.stringify(a.entry.doc.oplogVersion().toJSON());
			const aUpdatesBefore = server.updatesSeen.length;
			a.entry.session.setPresence({ blockId: 'cid:5@7:Map', anchor: 2, head: 4 });
			await waitFor(() => b.entry.session.remotePresenceList().length === 1, 'B sees A');
			const seen = b.entry.session.remotePresenceList()[0];
			assertEqual(seen.user.name, 'Alice', 'display name');
			assertEqual(seen.user.userId, 'attested-alice', "identity is the frame's actor");
			assertEqual(seen.attested, true, 'attested by the server');
			assertEqual(seen.cursor?.blockId, 'cid:5@7:Map', 'block id');
			assertEqual(seen.cursor?.head, 4, 'offset');
			assertEqual(
				JSON.stringify(a.entry.doc.oplogVersion().toJSON()),
				aVersion,
				"A's doc version unchanged"
			);
			assertEqual(
				JSON.stringify(b.entry.doc.toJSON()),
				JSON.stringify(a.entry.doc.toJSON()),
				'docs unchanged'
			);
			assertEqual(server.updatesSeen.length, aUpdatesBefore, 'no update frame from presence');
			assertEqual((await a.store.list(OBJECT_ID)).length, 0, 'no outbox entry from presence');
			assert(
				!JSON.stringify(b.entry.doc.toJSON()).includes('Alice'),
				'presence identity is not in the doc'
			);

			// Not a lock: B edits the block A is on, and it is accepted.
			edit(b.entry.doc, 'B types while A is present');
			await waitFor(() => server.accepted.length === 1, "B's edit accepted");
			await waitFor(
				() => a.entry.doc.getText('body').toString() === 'B types while A is present',
				'A receives it in place'
			);
			assertEqual(a.entry.session.remotePresenceList().length, 0, 'A sees no remote presence yet');

			// A connection loss clears what B knows about peers until they announce again.
			b.socket.serverClose(1006, '');
			await waitFor(() => b.entry.session.remotePresenceList().length === 0, 'cleared on loss');
		} finally {
			await a.repository.close(OBJECT_ID);
			await b.repository.close(OBJECT_ID);
		}
	}
);

restore();

export const result = suite.result();

if (import.meta.main) finish(result);
