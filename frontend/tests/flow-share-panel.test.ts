/**
 * Share panel (`contracts/ui-surface-v1.md` "职责边界" Share panel, `ADR-0012`,
 * `rest-api-v1.md` `GET/PUT .../grants`, `PUT .../inheritance`).
 *
 * Checks the real `flowApi` request shapes against a mocked `globalThis.fetch`, then drives
 * `FlowSharePanelController` with injected reads/commands:
 * - every downgrade (remove, lower a level, clear, break inheritance) is a `dry_run:true` request
 *   first, and the real request is only sent by `confirm()`;
 * - adding and raising grants (and restoring inheritance) are sent directly, no dry run;
 * - a self-lockout needs the user's acknowledgement before `confirm_self_lockout:true` is sent;
 * - the preview is not authorization: the receipt is the commit response and the roster is
 *   re-read from the server;
 * - a caller without `full_access` gets no roster, no directory read and no write;
 * - one key per intent across dry run, confirm and retries;
 * - errors branch on `error_code` / `details.action` only, never on `message`.
 *
 * Run standalone: `bun tests/flow-share-panel.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	FlowSharePanelController,
	SHARE_LEVEL_KEYS,
	SHARE_PRINCIPAL_KIND_KEYS,
	changeLowersPermissions,
	isGrantDowngrade,
	type ShareDirectoryEntry
} from '../src/lib/flow/share-panel';
import {
	flowApi,
	type FlowGrantsView,
	type FlowPermissionChanges,
	type FlowSetGrantsResponse,
	type FlowSetInheritanceResponse,
	type SetFlowGrantsInput,
	type SetFlowInheritanceInput
} from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const OBJECT = 'obj-1';
const WS = 'ws-1';
const ALICE = 'user-alice';
const BOB = 'user-bob';

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function view(extra: Partial<FlowGrantsView> = {}): FlowGrantsView {
	return {
		items: [
			{
				principal_kind: 'user',
				principal_id: ALICE,
				level: 'edit',
				granted_by: 'owner',
				created_at: '2026-10-10T00:00:00Z'
			},
			{
				principal_kind: 'user',
				principal_id: BOB,
				level: 'view',
				granted_by: 'owner',
				created_at: '2026-10-10T00:00:00Z'
			}
		],
		inherit_from_parent: true,
		inherited: [
			{ object_id: 'parent', principal_kind: 'user', principal_id: 'user-carol', level: 'comment' }
		],
		effective_level: 'full_access',
		...extra
	};
}

function changes(
	affected: FlowPermissionChanges['affected'] = [],
	loses = false
): FlowPermissionChanges {
	return {
		caller: {
			before_level: 'full_access',
			after_level: loses ? 'none' : 'full_access',
			loses_full_access: loses
		},
		affected
	};
}

type Call =
	| { kind: 'get' }
	| { kind: 'grants'; body: SetFlowGrantsInput }
	| { kind: 'inheritance'; body: SetFlowInheritanceInput }
	| { kind: 'directory' };

interface Harness {
	controller: FlowSharePanelController;
	calls: Call[];
	puts: () => Array<SetFlowGrantsInput | SetFlowInheritanceInput>;
}

function harness(options: {
	gets?: Array<ApiResult<FlowGrantsView>>;
	grants?: Array<ApiResult<FlowSetGrantsResponse>>;
	inheritance?: Array<ApiResult<FlowSetInheritanceResponse>>;
	directory?: ShareDirectoryEntry[];
}): Harness {
	const calls: Call[] = [];
	let n = 0;
	const gets = options.gets ?? [ok(view())];
	const grants = options.grants ?? [
		ok({ applied: true, event_id: 'evt-1', permission_changes: changes() })
	];
	const inheritance = options.inheritance ?? [
		ok({
			applied: true,
			event_id: 'evt-2',
			inherit_from_parent: false,
			permission_changes: changes()
		})
	];
	const next = <T>(queue: T[]): T => (queue.length > 1 ? (queue.shift() as T) : queue[0]);
	const controller = new FlowSharePanelController(OBJECT, WS, () => {}, {
		reads: {
			getGrants: async (objectId) => {
				assertEqual(objectId, OBJECT, 'grants read object id');
				calls.push({ kind: 'get' });
				return next(gets);
			}
		},
		commands: {
			setGrants: async (objectId, body) => {
				assertEqual(objectId, OBJECT, 'grants write object id');
				calls.push({ kind: 'grants', body: structuredClone(body) });
				return next(grants);
			},
			setInheritance: async (objectId, body) => {
				assertEqual(objectId, OBJECT, 'inheritance write object id');
				calls.push({ kind: 'inheritance', body: structuredClone(body) });
				return next(inheritance);
			}
		},
		directory: async () => {
			calls.push({ kind: 'directory' });
			return options.directory ?? [{ principal_kind: 'user', principal_id: ALICE, label: 'Alice' }];
		},
		newKey: () => {
			n += 1;
			return `key-${n}`;
		}
	});
	const puts = () =>
		calls.flatMap((call) =>
			call.kind === 'grants' || call.kind === 'inheritance' ? [call.body] : []
		);
	return { controller, calls, puts };
}

const selfLockout: ApiResult<FlowSetGrantsResponse> = {
	code: 403,
	message: 'irrelevant prose',
	data: null,
	error_code: 'policy_rejected',
	details: {
		action: 'authz_self_lockout',
		caller: { before_level: 'full_access', after_level: 'view', loses_full_access: true }
	}
};

const suite = new Suite('flow-share-panel');

await suite.checkAsync(
	'flowApi grants/inheritance use the contract paths, verbs and bodies',
	async () => {
		const seen: Array<{ method: string; path: string; body: unknown }> = [];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			const raw = String(input);
			seen.push({
				method: init?.method ?? 'GET',
				path: raw.startsWith('http') ? new URL(raw).pathname + new URL(raw).search : raw,
				body: init?.body ? JSON.parse(String(init.body)) : null
			});
			return new Response(JSON.stringify({ code: 0, message: 'ok', data: {} }), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		try {
			await flowApi.getGrants(OBJECT);
			await flowApi.setGrants(OBJECT, {
				grants: [{ principal_kind: 'user', principal_id: ALICE, level: 'view' }],
				confirm_self_lockout: false,
				dry_run: true,
				idempotency_key: 'k1'
			});
			await flowApi.setInheritance(OBJECT, {
				inherit_from_parent: false,
				confirm_self_lockout: false,
				dry_run: false,
				idempotency_key: 'k2'
			});
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			seen,
			[
				{ method: 'GET', path: `/api/v1/flow/objects/${OBJECT}/grants`, body: null },
				{
					method: 'PUT',
					path: `/api/v1/flow/objects/${OBJECT}/grants`,
					body: {
						grants: [{ principal_kind: 'user', principal_id: ALICE, level: 'view' }],
						confirm_self_lockout: false,
						dry_run: true,
						idempotency_key: 'k1'
					}
				},
				{
					method: 'PUT',
					path: `/api/v1/flow/objects/${OBJECT}/inheritance`,
					body: {
						inherit_from_parent: false,
						confirm_self_lockout: false,
						dry_run: false,
						idempotency_key: 'k2'
					}
				}
			],
			'GET grants, PUT grants, PUT inheritance (no initial_grants field at all)'
		);
	}
);

suite.check('downgrade detection: remove / lower / clear / break inheritance only', () => {
	const roster = view().items;
	assert(isGrantDowngrade(roster, roster.slice(1)), 'removing a principal is a downgrade');
	assert(
		isGrantDowngrade(roster, [{ ...roster[0], level: 'comment' }, roster[1]]),
		'lowering a level is a downgrade'
	);
	assert(isGrantDowngrade(roster, []), 'clearing is a downgrade');
	assert(
		!isGrantDowngrade(roster, [
			...roster,
			{ principal_kind: 'user', principal_id: 'x', level: 'view' }
		]),
		'adding is not'
	);
	assert(
		!isGrantDowngrade(roster, [{ ...roster[0], level: 'full_access' }, roster[1]]),
		'raising is not'
	);
	assert(
		changeLowersPermissions(view(), { kind: 'inheritance', inherit_from_parent: false }),
		'break'
	);
	assert(
		!changeLowersPermissions(view({ inherit_from_parent: false }), {
			kind: 'inheritance',
			inherit_from_parent: true
		}),
		'restoring inheritance is not a downgrade'
	);
	assert(
		changeLowersPermissions(view(), {
			kind: 'add',
			principal_kind: 'user',
			principal_id: ALICE,
			level: 'view'
		}),
		'"adding" an existing principal at a lower level is a downgrade'
	);
});

await suite.checkAsync(
	'every downgrade goes through dry_run first; confirm sends the same key',
	async () => {
		const cases = [
			{ kind: 'remove', principal_kind: 'user', principal_id: BOB } as const,
			{ kind: 'set_level', principal_kind: 'user', principal_id: ALICE, level: 'comment' } as const,
			{ kind: 'clear' } as const,
			{ kind: 'inheritance', inherit_from_parent: false } as const
		];
		for (const change of cases) {
			const h = harness({
				grants: [ok({ applied: false, event_id: null, permission_changes: changes() })],
				inheritance: [
					ok({
						applied: false,
						event_id: null,
						inherit_from_parent: false,
						permission_changes: changes()
					})
				]
			});
			await h.controller.load();
			const outcome = await h.controller.propose(change);
			assertEqual(outcome.status, 'previewed', `${change.kind}: stops at the preview`);
			assertEqual(h.puts().length, 1, `${change.kind}: exactly one request before confirm`);
			assertEqual(h.puts()[0].dry_run, true, `${change.kind}: the first request is a dry run`);
			assertEqual(
				h.puts()[0].confirm_self_lockout,
				false,
				`${change.kind}: no pre-filled lockout confirm`
			);
			assertEqual(
				h.controller.snapshot.pending?.phase,
				'confirm',
				`${change.kind}: waiting for confirm`
			);
			await h.controller.confirm();
			const [preview, commit] = h.puts();
			assertEqual(commit?.dry_run, false, `${change.kind}: confirm sends the real request`);
			assertEqual(commit?.idempotency_key, preview.idempotency_key, `${change.kind}: same key`);
			assertDeepEqual(
				{ ...commit, dry_run: true },
				preview,
				`${change.kind}: the confirmed body is the previewed body`
			);
		}
	}
);

await suite.checkAsync('removal and clear send the full replacement roster', async () => {
	const h = harness({});
	await h.controller.load();
	await h.controller.propose({ kind: 'remove', principal_kind: 'user', principal_id: BOB });
	assertDeepEqual(
		(h.puts()[0] as SetFlowGrantsInput).grants,
		[{ principal_kind: 'user', principal_id: ALICE, level: 'edit' }],
		'remove keeps every other row'
	);
	h.controller.cancel();
	await h.controller.propose({ kind: 'clear' });
	assertDeepEqual((h.puts()[1] as SetFlowGrantsInput).grants, [], 'clear sends []');
	assertEqual(h.puts()[1].dry_run, true, 'clear is previewed');
	assertEqual(h.puts()[1].idempotency_key, 'key-2', 'a new intent gets a new key');
});

await suite.checkAsync(
	'adding or raising a grant (and restoring inheritance) is sent directly',
	async () => {
		const h = harness({});
		await h.controller.load();
		const added = await h.controller.propose({
			kind: 'add',
			principal_kind: 'user',
			principal_id: 'user-dan',
			level: 'view'
		});
		assertEqual(added.status, 'committed', 'add commits without a preview');
		assertEqual(h.puts().length, 1, 'one request');
		assertEqual(h.puts()[0].dry_run, false, 'not a dry run');
		assertDeepEqual(
			(h.puts()[0] as SetFlowGrantsInput).grants,
			[
				{ principal_kind: 'user', principal_id: ALICE, level: 'edit' },
				{ principal_kind: 'user', principal_id: BOB, level: 'view' },
				{ principal_kind: 'user', principal_id: 'user-dan', level: 'view' }
			],
			'roster plus the new row'
		);
		const raised = await h.controller.propose({
			kind: 'set_level',
			principal_kind: 'user',
			principal_id: BOB,
			level: 'edit'
		});
		assertEqual(raised.status, 'committed', 'raise commits directly');
		assertEqual(h.puts()[1].dry_run, false, 'raise is not previewed');

		const h2 = harness({ gets: [ok(view({ inherit_from_parent: false }))] });
		await h2.controller.load();
		await h2.controller.propose({ kind: 'inheritance', inherit_from_parent: true });
		assertDeepEqual(
			h2.puts(),
			[
				{
					inherit_from_parent: true,
					confirm_self_lockout: false,
					dry_run: false,
					idempotency_key: 'key-1'
				}
			],
			'restoring inheritance is one direct request'
		);
	}
);

await suite.checkAsync(
	'self-lockout requires the acknowledgement before confirm_self_lockout',
	async () => {
		const h = harness({
			grants: [
				selfLockout,
				ok({ applied: false, event_id: null, permission_changes: changes([], true) }),
				ok({ applied: true, event_id: 'evt-9', permission_changes: changes([], true) })
			],
			gets: [ok(view()), ok(view({ effective_level: 'view', items: [], inherited: [] }))]
		});
		await h.controller.load();
		await h.controller.propose({ kind: 'clear' });
		const pending = h.controller.snapshot.pending;
		assert(pending?.selfLockout === true, 'the policy_rejected self-lockout is recognised');
		assertEqual(pending?.caller?.after_level, 'view', 'caller summary comes from details');
		assertEqual(pending?.acknowledged, false, 'never pre-acknowledged');
		assertEqual(
			(await h.controller.confirm()).status,
			'refused',
			'confirm without the tick is refused'
		);
		assertEqual(h.puts().length, 1, 'and sends nothing');
		await h.controller.acknowledgeSelfLockout(true);
		assertEqual(h.puts().length, 2, 'ticking re-previews');
		assertDeepEqual(
			[h.puts()[1].dry_run, h.puts()[1].confirm_self_lockout, h.puts()[1].idempotency_key],
			[true, true, 'key-1'],
			'second preview: dry run with the user-produced confirm, same key'
		);
		assertEqual(
			(await h.controller.confirm()).status,
			'committed',
			'confirm after the tick commits'
		);
		assertDeepEqual(
			[h.puts()[2].dry_run, h.puts()[2].confirm_self_lockout, h.puts()[2].idempotency_key],
			[false, true, 'key-1'],
			'commit carries confirm_self_lockout:true under the same key'
		);
	}
);

await suite.checkAsync(
	'a commit that newly trips self-lockout returns to confirm, same key, no tick',
	async () => {
		const h = harness({
			grants: [ok({ applied: false, event_id: null, permission_changes: changes() }), selfLockout]
		});
		await h.controller.load();
		await h.controller.propose({ kind: 'remove', principal_kind: 'user', principal_id: BOB });
		const outcome = await h.controller.confirm();
		assertEqual(outcome.status, 'previewed', 'back to the confirm step');
		const pending = h.controller.snapshot.pending;
		assertDeepEqual(
			[pending?.phase, pending?.selfLockout, pending?.acknowledged, pending?.key],
			['confirm', true, false, 'key-1'],
			'self-lockout now needs the tick; key unchanged'
		);
		assertEqual(
			(await h.controller.confirm()).status,
			'refused',
			'and confirm is refused until ticked'
		);
	}
);

await suite.checkAsync(
	'the preview is not authorization: receipt and roster come from the server',
	async () => {
		const previewSays = changes([
			{ principal_kind: 'user', principal_id: BOB, before_level: 'view', after_level: 'none' }
		]);
		const commitSays = changes([
			{ principal_kind: 'user', principal_id: BOB, before_level: 'edit', after_level: 'none' },
			{ principal_kind: 'user', principal_id: ALICE, before_level: 'edit', after_level: 'comment' }
		]);
		const serverAfter = view({
			items: [
				{
					principal_kind: 'user',
					principal_id: ALICE,
					level: 'comment',
					granted_by: 'someone-else',
					created_at: '2026-10-10T00:00:01Z'
				}
			]
		});
		const h = harness({
			gets: [ok(view()), ok(serverAfter)],
			grants: [
				ok({ applied: false, event_id: null, permission_changes: previewSays }),
				ok({ applied: true, event_id: 'evt-real', permission_changes: commitSays })
			]
		});
		await h.controller.load();
		await h.controller.propose({ kind: 'remove', principal_kind: 'user', principal_id: BOB });
		assertDeepEqual(
			h.controller.snapshot.pending?.preview,
			previewSays,
			'preview shown while confirming'
		);
		await h.controller.confirm();
		const snap = h.controller.snapshot;
		assertEqual(snap.pending, null, 'the preview is dropped after commit');
		assertDeepEqual(
			snap.receipt,
			{ eventId: 'evt-real', changes: commitSays },
			'receipt is the commit response'
		);
		assertEqual(
			h.calls.filter((call) => call.kind === 'get').length,
			2,
			'roster re-read after commit'
		);
		assert(snap.load.status === 'ready', 'ready');
		assertDeepEqual(
			snap.load.view,
			serverAfter,
			'roster is the server re-read, not a local application'
		);
	}
);

await suite.checkAsync('without full_access: no roster, no directory, no writes', async () => {
	const leaky = view({ effective_level: 'view' });
	const h = harness({ gets: [ok(leaky)] });
	const state = await h.controller.load();
	assert(state.status === 'ready', 'ready');
	assertEqual(state.canManage, false, 'cannot manage');
	assertDeepEqual(state.view.items, [], 'explicit roster dropped even if the response carried one');
	assertDeepEqual(state.view.inherited, [], 'inherited roster dropped');
	assertEqual(state.view.effective_level, 'view', 'own effective level kept');
	assertEqual(state.view.inherit_from_parent, true, 'inheritance flag kept');
	assertEqual(
		h.calls.some((call) => call.kind === 'directory'),
		false,
		'member directory not requested'
	);
	for (const change of [
		{ kind: 'clear' } as const,
		{ kind: 'add', principal_kind: 'user', principal_id: 'x', level: 'view' } as const,
		{ kind: 'inheritance', inherit_from_parent: false } as const
	]) {
		assertEqual((await h.controller.propose(change)).status, 'refused', `${change.kind} refused`);
	}
	assertEqual(h.puts().length, 0, 'no PUT was sent');

	const manager = harness({});
	await manager.controller.load();
	assertEqual(
		manager.calls.filter((call) => call.kind === 'directory').length,
		1,
		'a manager reads the directory'
	);
});

await suite.checkAsync('a retry of the same intent reuses its key', async () => {
	const unavailable: ApiResult<FlowSetGrantsResponse> = {
		code: 500,
		message: 'x',
		data: null,
		error_code: 'server_draining',
		details: { reason: 'contention', retry_after_ms: 10 }
	};
	const h = harness({
		grants: [
			ok({ applied: false, event_id: null, permission_changes: changes() }),
			unavailable,
			ok({ applied: true, event_id: 'e', permission_changes: changes() })
		]
	});
	await h.controller.load();
	await h.controller.propose({ kind: 'remove', principal_kind: 'user', principal_id: BOB });
	const failed = await h.controller.confirm();
	assert(failed.status === 'failed', 'first confirm fails');
	assertEqual(failed.failure.messageKey, 'flow.error.server_draining.contention', 'typed key');
	assertEqual(failed.failure.retryable, true, 'retryable');
	assertEqual((await h.controller.confirm()).status, 'committed', 'retry commits');
	assertDeepEqual(
		h.puts().map((body) => body.idempotency_key),
		['key-1', 'key-1', 'key-1'],
		'dry run, failed confirm and retry share one key'
	);
});

await suite.checkAsync(
	'errors branch on error_code and details.action, never on message',
	async () => {
		const fake: ApiResult<FlowSetGrantsResponse> = {
			code: 403,
			message: 'policy_rejected authz_self_lockout confirm_self_lockout',
			data: null,
			error_code: 'forbidden'
		};
		const h = harness({ grants: [fake] });
		await h.controller.load();
		const outcome = await h.controller.propose({ kind: 'clear' });
		assert(outcome.status === 'failed', 'a forbidden dry run fails');
		assertEqual(outcome.failure.messageKey, 'flow.error.forbidden', 'mapped from error_code');
		assertEqual(
			h.controller.snapshot.pending?.selfLockout,
			false,
			'message text does not make a lockout'
		);

		const otherAction = harness({
			grants: [{ ...selfLockout, details: { action: 'something_else' } }]
		});
		await otherAction.controller.load();
		const other = await otherAction.controller.propose({ kind: 'clear' });
		assert(other.status === 'failed', 'policy_rejected for another action is a failure');
		assertEqual(
			other.failure.messageKey,
			'flow.error.policy_rejected',
			'generic policy_rejected key'
		);

		const legacy = harness({ gets: [{ code: 404, message: 'flow object not found', data: null }] });
		const state = await legacy.controller.load();
		assert(state.status === 'failed', 'legacy 404 read fails');
		assertEqual(state.failure.messageKey, 'flow.error.not_found', 'legacy numeric code mapping');
	}
);

await suite.checkAsync('every share i18n key exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		const missing = [
			...Object.values(SHARE_LEVEL_KEYS),
			...Object.values(SHARE_PRINCIPAL_KIND_KEYS),
			'flow.share.level.unknown',
			'flow.share.error.rejected',
			'flow.share.error.unavailable'
		].filter((key) => typeof lookup(key) !== 'string');
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
	}
});

finish(suite.result());
