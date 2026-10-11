/**
 * vF `offline_accepted_zero_loss_and_recovery_draft` producer (`gates/vF-frontend-gate.yaml`
 * artifacts `offline_recovery_result`, "当前无 producer").
 *
 * Judged against `versions/v0.5-collaboration.md` "Web 交互" and "验收与回滚" ("离线 accepted edit
 * 零丢失，rejected intent 可重放或可导出") and `ui-surface-v1.md` "连接、refresh 与恢复状态机" steps
 * 4-5. The real `FlowObjectRepository` + `LoroObjectSession` + `FlowOfflineSync` run against a fake
 * IndexedDB (`fake-indexeddb`) and a fake collaboration server holding a real `loro-crdt` doc
 * (`tests/support/collab-fakes.ts`). Scripts:
 *
 *  A. disconnect -> N=5 edits offline (+1 peer edit lands on the server meanwhile) -> reconnect;
 *     the server refuses edit 3 (`invalid_update`). Edits 1-2 must be accepted under their original
 *     keys, edit 3 must be in the draft with the server's reason, edits 4-5 must be in the draft as
 *     its dependents without ever reaching the wire, and the client doc must equal the server doc.
 *  B. an edit commits on the server but the connection drops before `accepted` (write_state
 *     unknown). The recovery pass replays it under the SAME update_id and the server answers the
 *     replay with the original receipt (`collab-protocol-v1.md` "同 `update_id` 重放必须被答复");
 *     it is committed exactly once and acknowledged from that answer.
 *  C. edits offline, then the user is lowered to `view`; on reconnect the policy re-check refuses
 *     the replay: all edits go to the draft, the session is read-only, nothing is sent.
 *
 * The result is written to `<SYLVODE_SCRATCH_ROOT>/frontend/offline-recovery-result.json`, by
 * default the repository root's `.flow-gate/cache/frontend/` (git-ignored; the same scratch root
 * as the v0.4 producer and `scripts/*.sh`'s `SYLVODE_SCRATCH`), shaped for the vF artifact `evidence/vF/offline-recovery-result.json`.
 *
 * Run standalone: `bun tests/flow-offline-recovery.test.ts`
 */

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { createHash } from 'node:crypto';
import { execSync } from 'node:child_process';
import { LoroDoc } from 'loro-crdt';
import { Suite, assert, assertDeepEqual, assertEqual, finish, waitFor } from './support/harness';
import {
	DOCUMENT_ID,
	FakeCollabServer,
	FakeConnectivity,
	OBJECT_ID,
	WORKSPACE_ID,
	edit,
	freshIndexedDb,
	installFakes,
	objectView,
	textOf,
	OUTBOX_DB
} from './support/collab-fakes';
import { IndexedDbOutboxStore } from '../src/lib/flow/outbox';
import type { FlowObjectView } from '../src/lib/api/flow';

const suite = new Suite('offline_accepted_zero_loss_and_recovery_draft');
// `scripts/*.sh`: `SYLVODE_SCRATCH="${SYLVODE_SCRATCH_ROOT:-<checkout>/.flow-gate/cache}"`.
const SCRATCH_ROOT =
	process.env.SYLVODE_SCRATCH_ROOT ??
	join(dirname(new URL(import.meta.url).pathname), '..', '..', '.flow-gate/cache');
const OUT = join(SCRATCH_ROOT, 'frontend', 'offline-recovery-result.json');

interface ScenarioResult {
	name: string;
	edits_offline: number;
	enqueued_update_ids: string[];
	accepted_update_ids: string[];
	draft: Array<{ update_id: string; code: string; source: string; root_update_id?: string }>;
	lost_update_ids: string[];
	replay_keys_reused: boolean;
	never_sent_dependents: boolean;
	client_doc_equals_server_doc: boolean;
	client_text: string;
	server_text: string;
	pending_after: number;
	server_pending_accepted: number;
	sync_state_after: string;
	passed: boolean;
}

const scenarios: ScenarioResult[] = [];

/** JSON with object keys sorted, so two docs with the same content compare equal regardless of
 * the order their root containers were created in. */
function canonical(value: unknown): string {
	const sort = (v: unknown): unknown => {
		if (Array.isArray(v)) return v.map(sort);
		if (v && typeof v === 'object') {
			return Object.fromEntries(
				Object.keys(v as Record<string, unknown>)
					.sort()
					.map((k) => [k, sort((v as Record<string, unknown>)[k])])
			);
		}
		return v;
	};
	return JSON.stringify(sort(value));
}

async function setup(server: FakeCollabServer) {
	const factory = await freshIndexedDb();
	const restore = await installFakes(server);
	const store = await IndexedDbOutboxStore.open(factory, OUTBOX_DB);
	assert(store, 'fake IndexedDB opens');
	const connectivity = new FakeConnectivity();
	const { FlowObjectRepository } = await import('../src/lib/flow/object-repository');
	const repository = new FlowObjectRepository({
		outboxStore: store,
		session: { connectivity, answerTimeoutMs: 1500 },
		offlineSync: { sleep: (ms) => new Promise((r) => setTimeout(r, Math.min(ms, 20))) }
	});
	const entry = await repository.open({
		workspaceId: WORKSPACE_ID,
		objectId: OBJECT_ID,
		signal: new AbortController().signal,
		object: objectView() as unknown as FlowObjectView
	});
	const states: string[] = [];
	entry.session.state.subscribe((s) => states.push(s));
	let pending = 0;
	entry.pending.subscribe((n) => (pending = n));
	const enqueued: string[] = [];
	return {
		store,
		connectivity,
		repository,
		entry,
		states,
		pending: () => pending,
		enqueued,
		restore,
		async close() {
			await repository.close(OBJECT_ID);
			restore();
		}
	};
}

function serverText(server: FakeCollabServer): string {
	return server.doc.getText('body').toString();
}

async function offlineEdits(
	ctx: Awaited<ReturnType<typeof setup>>,
	texts: string[]
): Promise<string[]> {
	ctx.connectivity.goOffline();
	await waitFor(() => ctx.states.at(-1) === 'offline', 'indicator shows offline');
	for (const text of texts) edit(ctx.entry.doc, text);
	await waitFor(() => ctx.pending() === texts.length, `${texts.length} pending`);
	return (await ctx.store.list(OBJECT_ID)).map((e) => e.updateId);
}

// ---- A ------------------------------------------------------------------------------------------

await suite.checkAsync(
	'A: offline edits replay in order under their keys; a refusal and its dependents go to the draft',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await setup(server);
		try {
			edit(ctx.entry.doc, 'base.');
			await waitFor(
				() => ctx.pending() === 0 && server.accepted.length === 1,
				'online edit accepted'
			);
			const onlineIds = server.accepted.map((a) => a.updateId);

			const offlineIds = await offlineEdits(ctx, ['one.', 'two.', 'three.', 'four.', 'five.']);
			assertEqual(server.updatesSeen.length, 1, 'nothing reached the server while offline');
			assertEqual(ctx.states.at(-1), 'offline', 'offline state');

			// A peer edit lands while this client is away.
			const peer = new LoroDoc();
			peer.import(server.doc.export({ mode: 'snapshot' }));
			const peerUpdates: Uint8Array[] = [];
			peer.subscribeLocalUpdates((b: Uint8Array) => peerUpdates.push(b));
			peer.getText('peer').insert(0, 'from-peer');
			peer.commit();
			server.commitFromPeer(peerUpdates[0]);

			const refused = offlineIds[2];
			server.policy = (seen) =>
				seen.updateId === refused
					? {
							kind: 'reject',
							reject: { code: 'invalid_update', details: { reason: 'not_this_one' } }
						}
					: { kind: 'accept' };
			server.setOnline(true);
			ctx.connectivity.goOnline();

			await waitFor(
				() => ctx.pending() === 0 && ctx.entry.offline.draftEntries.length === 3,
				'replay finished with three drafted intents',
				5000
			);
			await ctx.entry.offline.idle();

			const acceptedIds = server.accepted
				.map((a) => a.updateId)
				.filter((id) => !id.startsWith('peer-'));
			assertDeepEqual(
				acceptedIds,
				[...onlineIds, offlineIds[0], offlineIds[1]],
				'accepted in enqueue order'
			);
			const sentOffline = server.updatesSeen.slice(1).map((s) => s.updateId);
			assertDeepEqual(
				sentOffline,
				[offlineIds[0], offlineIds[1], offlineIds[2]],
				'replay order; dependents never sent'
			);
			const keysReused = server.updatesSeen
				.slice(1)
				.every((s, i) => s.updateId === offlineIds[i] && s.idempotencyKey === offlineIds[i]);
			assert(keysReused, 'replay reused the outbox update_id/idempotency key');

			const draft = ctx.entry.offline.draftEntries.map((d) => ({
				update_id: d.updateId,
				code: d.reason.code,
				source: d.reason.source,
				...(d.reason.rootUpdateId ? { root_update_id: d.reason.rootUpdateId } : {})
			}));
			assertDeepEqual(
				draft.map((d) => [d.update_id, d.code, d.source]),
				[
					[offlineIds[2], 'invalid_update', 'server'],
					[offlineIds[3], 'invalid_update', 'dependency'],
					[offlineIds[4], 'invalid_update', 'dependency']
				],
				'the refusal and its two dependents are in the draft'
			);
			const all = [...onlineIds, ...offlineIds];
			const lost = all.filter(
				(id) => !acceptedIds.includes(id) && !draft.some((d) => d.update_id === id)
			);
			assertEqual(lost.length, 0, 'zero loss: every intent is accepted or in the draft');
			assertEqual(
				server.pendingAccepted,
				0,
				'the server never stored a causally incomplete update'
			);
			const clientJson = canonical(ctx.entry.doc.toJSON());
			const serverJson = canonical(server.doc.toJSON());
			assertEqual(clientJson, serverJson, 'client doc == server accepted doc');
			assertEqual(textOf(ctx.entry.doc), 'base.one.two.', 'accepted text');
			assertEqual(serverText(server), 'base.one.two.', 'server text');

			// The draft exports as JSON and text and lists every refused intent with its reason.
			const blob = await ctx.repository.exportRecoveryDraft(OBJECT_ID);
			const document = JSON.parse(await blob.text()) as {
				schema: string;
				entries: Array<{ update_id: string; reason: { code: string; source: string } }>;
			};
			assertEqual(document.schema, 'sylvode.flow.recovery-draft.v1', 'draft schema');
			assertDeepEqual(
				document.entries.map((e) => e.update_id),
				[offlineIds[2], offlineIds[3], offlineIds[4]],
				'draft JSON lists the refused intents'
			);
			const text = ctx.repository.recoveryDraftText(OBJECT_ID, (key, values) =>
				values ? `${key}${JSON.stringify(values)}` : key
			);
			assert(text.includes('flow.recovery.textHeader{"count":3}'), 'draft text header counts 3');
			assertEqual(
				text.split('\n').filter((l) => l.startsWith('flow.recovery.textLine')).length,
				3,
				'one line per intent'
			);

			scenarios.push({
				name: 'disconnect_edit_reconnect_partial_rejection',
				edits_offline: offlineIds.length,
				enqueued_update_ids: all,
				accepted_update_ids: acceptedIds,
				draft,
				lost_update_ids: lost,
				replay_keys_reused: keysReused,
				never_sent_dependents: !server.updatesSeen.some(
					(s) => s.updateId === offlineIds[3] || s.updateId === offlineIds[4]
				),
				client_doc_equals_server_doc: clientJson === serverJson,
				client_text: textOf(ctx.entry.doc),
				server_text: serverText(server),
				pending_after: ctx.pending(),
				server_pending_accepted: server.pendingAccepted,
				sync_state_after: ctx.states.at(-1) ?? '',
				passed: true
			});
		} finally {
			await ctx.close();
		}
	}
);

// ---- B ------------------------------------------------------------------------------------------

await suite.checkAsync(
	'B: an edit committed before the drop is replayed under its id and answered with the original receipt',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await setup(server);
		try {
			let dropped = false;
			server.policy = () => {
				if (dropped) return { kind: 'accept' };
				dropped = true;
				return { kind: 'commit_then_drop' };
			};
			edit(ctx.entry.doc, 'committed-then-dropped.');
			await waitFor(() => server.accepted.length === 1, 'server committed the edit');
			const id = server.accepted[0].updateId;
			// The client never saw `accepted`; the session reconnects on its own backoff.
			await waitFor(() => ctx.pending() === 0, 'outbox drained after the reconnect', 8000);
			await ctx.entry.offline.idle();
			assertEqual(server.accepted.length, 1, 'committed exactly once');
			assertEqual(server.updatesSeen.length, 2, 'replayed once after the drop');
			assertEqual(server.updatesSeen[1].updateId, id, 'the replay reuses the committed update_id');
			assertEqual(server.updatesSeen[1].idempotencyKey, id, 'and its idempotency key');
			assertEqual(server.replaysAnswered, 1, 'the server answered the replay');
			assertEqual(ctx.entry.offline.draftEntries.length, 0, 'nothing drafted');
			const clientJson = canonical(ctx.entry.doc.toJSON());
			const serverJson = canonical(server.doc.toJSON());
			assertEqual(clientJson, serverJson, 'client doc == server doc');
			scenarios.push({
				name: 'commit_before_drop_write_state_unknown',
				edits_offline: 0,
				enqueued_update_ids: [id],
				accepted_update_ids: [id],
				draft: [],
				lost_update_ids: [],
				replay_keys_reused: server.updatesSeen.every((s) => s.updateId === id),
				never_sent_dependents: true,
				client_doc_equals_server_doc: clientJson === serverJson,
				client_text: textOf(ctx.entry.doc),
				server_text: serverText(server),
				pending_after: ctx.pending(),
				server_pending_accepted: server.pendingAccepted,
				sync_state_after: ctx.states.at(-1) ?? '',
				passed: true
			});
		} finally {
			await ctx.close();
		}
	}
);

// ---- C ------------------------------------------------------------------------------------------

await suite.checkAsync(
	'C: lowered to view while offline: the policy re-check drafts every edit and nothing is sent',
	async () => {
		const server = new FakeCollabServer();
		const ctx = await setup(server);
		try {
			edit(ctx.entry.doc, 'kept.');
			await waitFor(
				() => server.accepted.length === 1 && ctx.pending() === 0,
				'online edit accepted'
			);
			const offlineIds = await offlineEdits(ctx, ['x.', 'y.', 'z.']);
			server.effectiveLevel = 'view';
			server.setOnline(true);
			ctx.connectivity.goOnline();
			await waitFor(() => ctx.entry.offline.draftEntries.length === 3, 'three drafted', 5000);
			await ctx.entry.offline.idle();
			await waitFor(() => ctx.states.at(-1) === 'read_only', 'read-only indicator');
			assertEqual(server.updatesSeen.length, 1, 'no offline edit reached the server');
			assertDeepEqual(
				ctx.entry.offline.draftEntries.map((d) => [d.updateId, d.reason.code, d.reason.source]),
				offlineIds.map((id) => [id, 'policy_rejected', 'precheck']),
				'every edit is drafted with the policy reason'
			);
			const clientJson = canonical(ctx.entry.doc.toJSON());
			const serverJson = canonical(server.doc.toJSON());
			assertEqual(clientJson, serverJson, 'rebuilt from the accepted snapshot');
			assertEqual(textOf(ctx.entry.doc), 'kept.', 'only accepted text remains on the canvas');
			let readOnly = false;
			ctx.entry.readOnly.subscribe((v) => (readOnly = v))();
			assertEqual(readOnly, true, 'the repository reports read-only');
			scenarios.push({
				name: 'permission_lowered_while_offline',
				edits_offline: offlineIds.length,
				enqueued_update_ids: [server.accepted[0].updateId, ...offlineIds],
				accepted_update_ids: [server.accepted[0].updateId],
				draft: ctx.entry.offline.draftEntries.map((d) => ({
					update_id: d.updateId,
					code: d.reason.code,
					source: d.reason.source
				})),
				lost_update_ids: [],
				replay_keys_reused: true,
				never_sent_dependents: server.updatesSeen.length === 1,
				client_doc_equals_server_doc: clientJson === serverJson,
				client_text: textOf(ctx.entry.doc),
				server_text: serverText(server),
				pending_after: ctx.pending(),
				server_pending_accepted: server.pendingAccepted,
				sync_state_after: ctx.states.at(-1) ?? '',
				passed: true
			});
		} finally {
			await ctx.close();
		}
	}
);

export const result = suite.result();

function sourceHead(): string {
	try {
		return execSync('git rev-parse HEAD', { encoding: 'utf8' }).trim();
	} catch {
		return 'unknown';
	}
}

const artifact = {
	schema_version: 'sylvode.flow.offline-recovery-result.v1',
	gate: 'offline_accepted_zero_loss_and_recovery_draft',
	source_head: sourceHead(),
	producer: 'frontend/tests/flow-offline-recovery.test.ts',
	harness: {
		outbox: 'fake-indexeddb (IndexedDB API)',
		transport: 'fake collab server (loro-crdt)',
		document_id: DOCUMENT_ID
	},
	scenarios,
	totals: {
		scenarios: scenarios.length,
		accepted: scenarios.reduce((n, s) => n + s.accepted_update_ids.length, 0),
		drafted: scenarios.reduce((n, s) => n + s.draft.length, 0),
		lost: scenarios.reduce((n, s) => n + s.lost_update_ids.length, 0)
	},
	checks: { passed: result.passed, failed: result.failed, skipped: result.skipped },
	passed:
		result.failed === 0 &&
		result.skipped === 0 &&
		scenarios.length === 3 &&
		scenarios.every(
			(s) => s.passed && s.lost_update_ids.length === 0 && s.client_doc_equals_server_doc
		)
};
const body = JSON.stringify(artifact, null, 2);
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, `${body}\n`);
console.log(
	`result written to ${OUT} (sha256 ${createHash('sha256').update(body).digest('hex').slice(0, 16)})`
);

if (import.meta.main) finish(result);
