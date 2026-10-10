/**
 * Workspace Flow operations panel (`/workspace/{id}/settings/flow/operations`;
 * `contracts/rest-api-v1.md` v0.8 table, task FP-N3).
 *
 * Checks the real `flowApi` request shapes (through the `FlowObjectRepository` /
 * `FlowCommandService` adapters) against a mocked `globalThis.fetch`, then drives the
 * `operations-service.ts` state machines with injected fakes: dry-run -> receipt -> execute
 * gating, verify being dry-run only, idempotency keys per intent, `error_code`-only branching,
 * the replay window and per-mode response parsing, and dead-letter accounting.
 *
 * Run standalone: `bun tests/flow-operations.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	assertNotEqual,
	finish
} from './support/harness';
import {
	DeliveryReplay,
	FlowOperationsService,
	HEALTH_STATUS_KEYS,
	HealthMonitor,
	INTEGRITY_STATUS_KEYS,
	MAINTENANCE_TITLE_KEYS,
	MaintenanceOperation,
	OPERATION_NAME_KEYS,
	ProjectionLagPager,
	RECEIPT_FIELD_KEYS,
	REPLAY_MAX_WINDOW_DAYS,
	REPLAY_MODE_KEYS,
	classifyOperationsError,
	deadLetterAlert,
	deadLetterTotal,
	parseReplayResponse,
	receiptRows,
	validateReplayForm,
	type HealthSnapshot,
	type ReplayForm
} from '../src/lib/flow/operations-service';
import { FlowCommandService } from '../src/lib/flow/command-service';
import { FlowObjectRepository } from '../src/lib/flow/object-repository';
import type { ApiResult } from '../src/lib/api/client';
import type {
	FlowAdminHealth,
	FlowOperationReceipt,
	FlowProjectionLag,
	FlowReplayResponse
} from '../src/lib/api/flow';

const WS = '11111111-1111-4111-8111-111111111111';
const DOC = 'abcdef22-2222-4222-8222-22222222abcd';
const OBJ = 'fedcba33-3333-4333-8333-33333333dcba';
const NOW = Date.parse('2026-10-09T12:00:00Z');
const HOUR = 3_600_000;
const DAY = 86_400_000;

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function err(code: number, extra: Partial<ApiResult<never>> = {}): ApiResult<never> {
	return { code, message: 'everything is fine', data: null, ...extra };
}

function receipt(
	operation: string,
	dryRun: boolean,
	head: number,
	result: Record<string, unknown> = {},
	id = `op-${operation}-${dryRun}-${head}`
): FlowOperationReceipt {
	return {
		operation_id: id,
		operation,
		status: 'completed',
		dry_run: dryRun,
		workspace_id: WS,
		object_id: OBJ,
		document_id: DOC,
		expected_head_seq: head,
		result
	};
}

function keys(): () => string {
	let n = 0;
	return () => `key-${++n}`;
}

interface Call {
	method: string;
	target: string;
	body: Record<string, unknown>;
}

/** Fake `FlowCommandService` with a scripted reply queue per method. */
function fakeCommands(replies: Record<string, Array<ApiResult<unknown>>>) {
	const calls: Call[] = [];
	const reply = (method: string, target: string, body: unknown) => {
		calls.push({ method, target, body: body as Record<string, unknown> });
		const queue = replies[method] ?? [];
		const next = queue.shift();
		if (!next) throw new Error(`unexpected ${method} call`);
		return Promise.resolve(next);
	};
	return {
		calls,
		commands: {
			verifyDocument: (id: string, body: unknown) =>
				reply('verify', id, body) as Promise<ApiResult<FlowOperationReceipt>>,
			compactDocument: (id: string, body: unknown) =>
				reply('compact', id, body) as Promise<ApiResult<FlowOperationReceipt>>,
			rebuildProjection: (id: string, body: unknown) =>
				reply('rebuild', id, body) as Promise<ApiResult<FlowOperationReceipt>>,
			replayDeliveries: (id: string, body: unknown) =>
				reply('replay', id, body) as Promise<ApiResult<FlowReplayResponse>>
		}
	};
}

function lagItem(objectId: string) {
	return { object_id: objectId, head_seq: 4, projection_seq: 0, lag: 4 };
}

function health(extra: Partial<FlowAdminHealth> = {}): FlowAdminHealth {
	return {
		status: 'healthy',
		connections: 2,
		accept_rate: 0.5,
		reject_rate: 0,
		queue_depth: 0,
		oldest_job_age: null,
		storage_bytes: 486,
		dead_letter: { dispatch_failed: 0, delivery_failed: 0, oldest_failed_age: null },
		delivery_cancelled: 0,
		...extra
	};
}

const suite = new Suite('flow-operations');

// ---- 1. wire shapes through the adapters ----------------------------------------------------

await suite.checkAsync(
	'adapters call the contract paths with the exact query strings and bodies',
	async () => {
		const seen: Array<{ method: string; path: string; body: unknown }> = [];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			const raw = String(input);
			const url = raw.startsWith('http') ? new URL(raw) : new URL(raw, 'http://x');
			seen.push({
				method: init?.method ?? 'GET',
				path: url.pathname + url.search,
				body: init?.body ? JSON.parse(String(init.body)) : null
			});
			return new Response(JSON.stringify({ code: 0, message: 'ok', data: {} }), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		try {
			const repo = new FlowObjectRepository();
			const commands = new FlowCommandService();
			await repo.getAdminHealth(WS);
			await repo.getAdminLag(WS);
			await repo.getAdminIntegrity(WS, { scope: 'documents', limit: 50 });
			await repo.getProjectionLag(WS, { cursor: 'c1', limit: 20 });
			await commands.verifyDocument(DOC, { dry_run: true, deep: true, idempotency_key: 'k1' });
			await commands.compactDocument(DOC, {
				dry_run: false,
				expected_head_seq: 4,
				confirm_document_id: DOC,
				idempotency_key: 'k2'
			});
			await commands.rebuildProjection(OBJ, {
				dry_run: true,
				expected_head_seq: 4,
				idempotency_key: 'k3'
			});
			await commands.replayDeliveries(WS, {
				mode: 'requeue_failed',
				from: '2026-10-09T10:00:00.000Z',
				to: '2026-10-09T11:00:00.000Z',
				dry_run: true,
				confirm: true,
				idempotency_key: 'k4'
			});
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			seen,
			[
				{ method: 'GET', path: `/api/v1/admin/workspaces/${WS}/flow/health`, body: null },
				{ method: 'GET', path: `/api/v1/admin/workspaces/${WS}/flow/lag`, body: null },
				{
					method: 'GET',
					path: `/api/v1/admin/workspaces/${WS}/flow/integrity?scope=documents&limit=50`,
					body: null
				},
				{
					method: 'GET',
					path: `/api/v1/workspaces/${WS}/flow/projection-lag?cursor=c1&limit=20`,
					body: null
				},
				{
					method: 'POST',
					path: `/api/v1/admin/flow/documents/${DOC}/verify`,
					body: { dry_run: true, deep: true, idempotency_key: 'k1' }
				},
				{
					method: 'POST',
					path: `/api/v1/admin/flow/documents/${DOC}/compact`,
					body: {
						dry_run: false,
						expected_head_seq: 4,
						confirm_document_id: DOC,
						idempotency_key: 'k2'
					}
				},
				{
					method: 'POST',
					path: `/api/v1/admin/flow/objects/${OBJ}/rebuild-projection`,
					body: { dry_run: true, expected_head_seq: 4, idempotency_key: 'k3' }
				},
				{
					method: 'POST',
					path: `/api/v1/admin/workspaces/${WS}/flow/deliveries/replay`,
					body: {
						mode: 'requeue_failed',
						from: '2026-10-09T10:00:00.000Z',
						to: '2026-10-09T11:00:00.000Z',
						dry_run: true,
						confirm: true,
						idempotency_key: 'k4'
					}
				}
			],
			'request log'
		);
	}
);

// ---- 2. verify is dry-run only ---------------------------------------------------------------

await suite.checkAsync('verify always sends dry_run:true and can never execute', async () => {
	const fake = fakeCommands({
		verify: [ok(receipt('verify_document', true, 0)), ok(receipt('verify_document', true, 7))]
	});
	const op = new MaintenanceOperation('verify', DOC, { commands: fake.commands, newKey: keys() });
	assertEqual(op.canDryRun(), true, 'verify without a head can run (head is optional)');
	await op.dryRun();
	op.setDeep(true);
	op.setExpectedHead('7');
	await op.dryRun();
	assertDeepEqual(
		fake.calls.map((call) => call.body),
		[
			{ dry_run: true, deep: false, idempotency_key: 'key-1' },
			{ dry_run: true, deep: true, expected_head_seq: 7, idempotency_key: 'key-2' }
		],
		'verify bodies'
	);
	op.setConfirmText(DOC);
	assertEqual(op.canExecute(), false, 'verify has no execute');
	assertEqual((await op.execute()).status, 'refused', 'verify execute refused');
	assertEqual(fake.calls.length, 2, 'no request beyond the two dry-runs');
	op.setExpectedHead('seven');
	assertEqual(op.canDryRun(), false, 'an invalid head blocks verify');
});

// ---- 3. execute gating -----------------------------------------------------------------------

await suite.checkAsync(
	'compact execute is refused with zero requests without a receipt, without a head, or with a mismatched confirmation',
	async () => {
		// (a) no receipt
		const a = fakeCommands({});
		const noReceipt = new MaintenanceOperation(
			'compact',
			DOC,
			{ commands: a.commands, newKey: keys() },
			3
		);
		noReceipt.setConfirmText(DOC);
		assertEqual(noReceipt.canExecute(), false, 'no receipt -> cannot execute');
		assertEqual((await noReceipt.execute()).status, 'refused', 'no receipt -> refused');
		assertEqual(a.calls.length, 0, 'no receipt -> zero requests');

		// (b) no expected head: the dry-run cannot even be sent, and clearing the head after a
		// dry-run voids the receipt.
		const b = fakeCommands({ compact: [ok(receipt('compact', true, 3))] });
		const noHead = new MaintenanceOperation('compact', DOC, {
			commands: b.commands,
			newKey: keys()
		});
		assertEqual(noHead.canDryRun(), false, 'compact dry-run needs a head');
		assertEqual((await noHead.dryRun()).status, 'refused', 'dry-run refused without head');
		noHead.setExpectedHead('3');
		await noHead.dryRun();
		noHead.setConfirmText(DOC);
		assertEqual(noHead.canExecute(), true, 'sanity: gate opens with all three');
		noHead.setExpectedHead('');
		assertEqual(noHead.canExecute(), false, 'blank head -> cannot execute');
		assertEqual(noHead.snapshot().preview, null, 'changing the head voids the receipt');
		assertEqual((await noHead.execute()).status, 'refused', 'blank head -> refused');
		assertEqual(b.calls.length, 1, 'only the dry-run was sent');

		// (c) confirmation mismatch
		const c = fakeCommands({ compact: [ok(receipt('compact', true, 3))] });
		const mismatch = new MaintenanceOperation(
			'compact',
			DOC,
			{ commands: c.commands, newKey: keys() },
			3
		);
		await mismatch.dryRun();
		for (const typed of ['', OBJ, DOC.toUpperCase(), DOC.slice(0, -1), `${DOC}x`]) {
			mismatch.setConfirmText(typed);
			assertEqual(mismatch.canExecute(), false, `confirm "${typed}" must not open the gate`);
			assertEqual((await mismatch.execute()).status, 'refused', `confirm "${typed}" refused`);
		}
		assertEqual(c.calls.length, 1, 'mismatched confirmations sent nothing');
	}
);

await suite.checkAsync(
	'compact execute sends the receipt head and the typed id under its own key; a retry reuses it',
	async () => {
		const fake = fakeCommands({
			compact: [
				err(503),
				ok(receipt('compact', true, 7, { would_compact: true })),
				err(500),
				ok(receipt('compact', false, 7, { deleted_updates: 3 }))
			]
		});
		const op = new MaintenanceOperation(
			'compact',
			DOC,
			{ commands: fake.commands, newKey: keys() },
			7
		);
		const first = await op.dryRun();
		assertEqual(first.status, 'failed', 'first dry-run 503');
		assertEqual(op.snapshot().retry, 'dry_run', '503 offers a dry-run retry');
		await op.dryRun();
		op.setConfirmText(`  ${DOC} `);
		assertEqual(op.canExecute(), true, 'gate open (surrounding whitespace ignored)');
		const failedExec = await op.execute();
		assertEqual(failedExec.status, 'failed', 'execute 500');
		assertEqual(op.snapshot().retry, 'execute', '500 offers an execute retry');
		assertEqual(op.canExecute(), true, 'retryable failure keeps the receipt');
		const done = await op.execute();
		assertEqual(done.status, 'ok', 'execute ok');
		assertDeepEqual(
			fake.calls.map((call) => [call.target, call.body]),
			[
				[DOC, { dry_run: true, expected_head_seq: 7, idempotency_key: 'key-1' }],
				[DOC, { dry_run: true, expected_head_seq: 7, idempotency_key: 'key-1' }],
				[
					DOC,
					{
						dry_run: false,
						expected_head_seq: 7,
						confirm_document_id: DOC,
						idempotency_key: 'key-2'
					}
				],
				[
					DOC,
					{
						dry_run: false,
						expected_head_seq: 7,
						confirm_document_id: DOC,
						idempotency_key: 'key-2'
					}
				]
			],
			'dry-run retry reuses key-1; execute uses its own key-2 and its retry reuses key-2'
		);
		assertNotEqual(
			fake.calls[2].body.idempotency_key,
			fake.calls[0].body.idempotency_key,
			'execute key differs from the dry-run key (server hashes dry_run into the body)'
		);
		assertEqual(op.snapshot().executed?.dry_run, false, 'executed receipt is the execute one');
		assertEqual(op.canExecute(), false, 'nothing left to execute');
		assertEqual(op.canDryRun(), false, 'a finished operation does not dry-run again');
	}
);

await suite.checkAsync(
	'execute uses the head the dry-run receipt was taken at, and a changed head voids it',
	async () => {
		const fake = fakeCommands({
			rebuild: [
				ok(receipt('rebuild_projection', true, 5)),
				ok(receipt('rebuild_projection', true, 6))
			]
		});
		const op = new MaintenanceOperation(
			'rebuild',
			OBJ,
			{ commands: fake.commands, newKey: keys() },
			5
		);
		await op.dryRun();
		op.setConfirmText(OBJ);
		op.setExpectedHead('6');
		assertEqual(op.canExecute(), false, 'head changed after dry-run -> receipt void');
		assertEqual((await op.execute()).status, 'refused', 'refused');
		await op.dryRun();
		assertEqual(fake.calls[1].body.idempotency_key, 'key-2', 'new head -> new dry-run intent key');
		assertEqual(op.canExecute(), true, 'fresh receipt at head 6 opens the gate');
	}
);

await suite.checkAsync(
	'rebuild execute sends confirm_object_id and renders before/after hash',
	async () => {
		const fake = fakeCommands({
			rebuild: [
				ok(
					receipt('rebuild_projection', true, 2, {
						before_hash: 'aa',
						after_hash: 'bb',
						changed: true
					})
				),
				ok(
					receipt('rebuild_projection', false, 2, {
						before_hash: 'aa',
						after_hash: 'bb',
						executed: true
					})
				)
			]
		});
		const op = new MaintenanceOperation(
			'rebuild',
			OBJ,
			{ commands: fake.commands, newKey: keys() },
			2
		);
		await op.dryRun();
		op.setConfirmText(OBJ);
		await op.execute();
		assertDeepEqual(
			fake.calls[1].body,
			{ dry_run: false, expected_head_seq: 2, confirm_object_id: OBJ, idempotency_key: 'key-2' },
			'rebuild execute body'
		);
		const rows = receiptRows(op.snapshot().executed as FlowOperationReceipt);
		assertDeepEqual(
			rows.map((row) => [row.field, row.labelKey, row.value]),
			[
				['before_hash', 'flow.operations.receipt.field.before_hash', 'aa'],
				['after_hash', 'flow.operations.receipt.field.after_hash', 'bb'],
				['executed', 'flow.operations.receipt.field.executed', 'true']
			],
			'receipt rows'
		);
		const nested = receiptRows(
			receipt('verify_document', true, 0, {
				deep: false,
				fingerprint: { semantic_hash: 'h', extra_field: 1 }
			})
		);
		assertDeepEqual(
			nested.map((row) => [row.field, row.labelKey]),
			[
				['deep', 'flow.operations.receipt.field.deep'],
				['fingerprint.semantic_hash', 'flow.operations.receipt.field.semantic_hash'],
				['fingerprint.extra_field', null]
			],
			'nested verify fingerprint rows; unknown field kept with no label'
		);
	}
);

await suite.checkAsync(
	'a dry-run answered with a non-dry-run receipt is never shown as a preview',
	async () => {
		const fake = fakeCommands({ compact: [ok(receipt('compact', false, 1))] });
		const op = new MaintenanceOperation(
			'compact',
			DOC,
			{ commands: fake.commands, newKey: keys() },
			1
		);
		const outcome = await op.dryRun();
		assertEqual(outcome.status, 'failed', 'rejected');
		assertEqual(op.snapshot().preview, null, 'no preview');
		assertEqual(op.snapshot().executed, null, 'not shown as executed either');
		assertEqual(op.snapshot().closed, true, 'closed');
	}
);

// ---- 4. error classification ------------------------------------------------------------------

await suite.checkAsync(
	'server_rejected is permanent: no retry, operation closed, nothing else sent',
	async () => {
		const fake = fakeCommands({
			compact: [
				ok(receipt('compact', true, 1)),
				err(500, { error_code: 'server_rejected', details: { write_state: 'not_applied' } })
			]
		});
		const op = new MaintenanceOperation(
			'compact',
			DOC,
			{ commands: fake.commands, newKey: keys() },
			1
		);
		await op.dryRun();
		op.setConfirmText(DOC);
		await op.execute();
		const snap = op.snapshot();
		assertEqual(snap.failure?.messageKey, 'flow.error.server_rejected', 'server_rejected key');
		assertEqual(snap.retry, null, 'no retry offered');
		assertEqual(snap.closed, true, 'closed');
		assertEqual(op.canExecute(), false, 'cannot execute');
		assertEqual(op.canDryRun(), false, 'cannot dry-run');
		assertEqual((await op.execute()).status, 'refused', 'refused');
		assertEqual(fake.calls.length, 2, 'nothing more sent');
	}
);

await suite.check('classification reads error_code / numeric code, never message', () => {
	assertEqual(
		classifyOperationsError(err(400, { error_code: 'forbidden', message: 'ok' })).kind,
		'forbidden',
		'typed forbidden wins over code 400'
	);
	const legacyForbiddenText = classifyOperationsError(err(500, { message: 'forbidden' }));
	assertEqual(legacyForbiddenText.kind, 'failed', 'message "forbidden" is not forbidden');
	assertEqual(legacyForbiddenText.retryable, true, 'legacy 5xx is retryable');
	const staleText = classifyOperationsError(err(409, { message: 'server_rejected' }));
	assertEqual(staleText.messageKey, 'flow.operations.error.conflict', 'legacy 409 -> conflict');
	assertEqual(staleText.permanent, false, 'a 409 is fixable by a new dry-run');
	assertEqual(
		classifyOperationsError(err(404)).messageKey,
		'flow.operations.error.notFound',
		'legacy 404'
	);
	assertEqual(classifyOperationsError(err(403)).kind, 'forbidden', 'legacy 403');
	const unknown = classifyOperationsError(err(400, { error_code: 'brand_new_code' }));
	assertEqual(unknown.permanent, true, 'unknown typed code is permanent');
	assertEqual(unknown.retryable, false, 'unknown typed code has no retry');
	const drain = classifyOperationsError(
		err(503, {
			error_code: 'server_draining',
			details: { reason: 'contention', retry_after_ms: 10 }
		})
	);
	assertEqual(drain.messageKey, 'flow.error.server_draining.contention', 'drain reason key');
	assertEqual(drain.retryable, true, 'drain is retryable');
});

await suite.checkAsync('a non-retryable execute failure voids the receipt', async () => {
	const fake = fakeCommands({
		compact: [ok(receipt('compact', true, 1)), err(409, { message: 'stale_frontier' })]
	});
	const op = new MaintenanceOperation(
		'compact',
		DOC,
		{ commands: fake.commands, newKey: keys() },
		1
	);
	await op.dryRun();
	op.setConfirmText(DOC);
	await op.execute();
	assertEqual(op.snapshot().preview, null, 'receipt cleared');
	assertEqual(op.canExecute(), false, 'must dry-run again');
	assertEqual(op.canDryRun(), true, 'and may');
	assertEqual(op.snapshot().retry, null, 'no retry button for a 409');
});

// ---- 5. delivery replay -----------------------------------------------------------------------

function form(extra: Partial<ReplayForm> = {}): ReplayForm {
	return {
		mode: 'rebuild',
		eventType: '',
		subscriberKind: '',
		subscriberId: '',
		from: new Date(NOW - HOUR).toISOString(),
		to: new Date(NOW).toISOString(),
		...extra
	};
}

await suite.check('replay window is validated locally against replay_max_window_days', () => {
	assertEqual(REPLAY_MAX_WINDOW_DAYS, 30, 'limits-v1.md value');
	const cases: Array<[Partial<ReplayForm>, string | null]> = [
		[{}, null],
		[{ from: new Date(NOW - 31 * DAY).toISOString() }, 'flow.operations.replay.error.tooOld'],
		[{ from: new Date(NOW - 30 * DAY).toISOString() }, 'flow.operations.replay.error.tooOld'],
		[{ from: new Date(NOW - 30 * DAY + 60_000).toISOString() }, null],
		[{ to: new Date(NOW + 60_000).toISOString() }, 'flow.operations.replay.error.future'],
		[{ from: new Date(NOW).toISOString() }, 'flow.operations.replay.error.empty'],
		[{ from: 'not a date' }, 'flow.operations.replay.error.invalidDate'],
		[{ subscriberId: 'abc' }, 'flow.operations.replay.error.invalidSubscriber']
	];
	for (const [patch, expected] of cases) {
		const result = validateReplayForm(form(patch), NOW);
		assertEqual(result.ok ? null : result.errorKey, expected, JSON.stringify(patch));
	}
});

await suite.checkAsync(
	'an out-of-window replay is refused locally with zero requests',
	async () => {
		const fake = fakeCommands({});
		const replay = new DeliveryReplay(
			WS,
			{ commands: fake.commands, newKey: keys(), now: () => NOW },
			form({
				from: new Date(NOW - 31 * DAY).toISOString()
			})
		);
		const outcome = await replay.dryRun();
		assertEqual(outcome.status, 'invalid', 'invalid');
		assertEqual(
			replay.snapshot().validationKey,
			'flow.operations.replay.error.tooOld',
			'tooOld key'
		);
		assertEqual(fake.calls.length, 0, 'no request');
	}
);

await suite.checkAsync(
	'replay: dry-run summary, explicit acknowledgement, execute of the same request under its own key',
	async () => {
		const rebuildData = {
			replayed: 0,
			skipped_already_delivered: 0,
			rebuilt_delivery_ids: [],
			window: { from: 'a', to: 'b' }
		};
		const fake = fakeCommands({
			replay: [
				ok(rebuildData),
				err(502),
				ok({ ...rebuildData, replayed: 2, rebuilt_delivery_ids: ['d1', 'd2'] })
			]
		});
		const replay = new DeliveryReplay(
			WS,
			{ commands: fake.commands, newKey: keys(), now: () => NOW },
			form({ eventType: ' flow.object.created ', subscriberKind: 'webhook' })
		);
		const dry = await replay.dryRun();
		assertEqual(dry.status, 'ok', 'dry-run ok');
		assertEqual(replay.canExecute(), false, 'not acknowledged -> no execute');
		assertEqual((await replay.execute()).status, 'refused', 'refused without acknowledgement');
		assertEqual(fake.calls.length, 1, 'only the dry-run');
		replay.acknowledge(true);
		assertEqual((await replay.execute()).status, 'failed', '502');
		assertEqual(replay.snapshot().retry, 'execute', 'retry offered');
		const done = await replay.execute();
		assertEqual(done.status, 'ok', 'executed');
		const expected = {
			mode: 'rebuild',
			event_type: 'flow.object.created',
			subscriber_kind: 'webhook',
			from: new Date(NOW - HOUR).toISOString(),
			to: new Date(NOW).toISOString(),
			confirm: true
		};
		assertDeepEqual(
			fake.calls.map((call) => call.body),
			[
				{ ...expected, dry_run: true, idempotency_key: 'key-1' },
				{ ...expected, dry_run: false, idempotency_key: 'key-2' },
				{ ...expected, dry_run: false, idempotency_key: 'key-2' }
			],
			'replay bodies and keys'
		);
		const executed = replay.snapshot().executed;
		assert(executed && executed.mode === 'rebuild', 'rebuild outcome');
		assertEqual(executed.replayed, 2, 'replayed count');
		assertDeepEqual(executed.deliveryIds, ['d1', 'd2'], 'rebuilt ids');
	}
);

await suite.checkAsync('changing the form voids the summary and the acknowledgement', async () => {
	const fake = fakeCommands({
		replay: [
			ok({
				requeued: 0,
				skipped_not_failed: 0,
				requeued_delivery_ids: [],
				window: { from: 'a', to: 'b' }
			})
		]
	});
	const replay = new DeliveryReplay(
		WS,
		{ commands: fake.commands, newKey: keys(), now: () => NOW },
		form({ mode: 'requeue_failed' })
	);
	await replay.dryRun();
	replay.acknowledge(true);
	assertEqual(replay.canExecute(), true, 'sanity');
	replay.setForm(form({ mode: 'requeue_failed', eventType: 'x' }));
	assertEqual(replay.snapshot().preview, null, 'summary cleared');
	assertEqual(replay.snapshot().acknowledged, false, 'acknowledgement cleared');
	assertEqual((await replay.execute()).status, 'refused', 'refused');
	assertEqual(fake.calls.length, 1, 'nothing sent');
});

await suite.check('replay responses are parsed per mode and mixed fields are a violation', () => {
	const window = { from: 'a', to: 'b' };
	const rebuild = {
		replayed: 1,
		skipped_already_delivered: 2,
		rebuilt_delivery_ids: ['x'],
		window
	};
	const requeue = { requeued: 3, skipped_not_failed: 4, requeued_delivery_ids: ['y'], window };
	const r1 = parseReplayResponse('rebuild', rebuild);
	assert(r1 && r1.mode === 'rebuild', 'rebuild parsed');
	assertDeepEqual(
		[r1.replayed, r1.skippedAlreadyDelivered, r1.deliveryIds],
		[1, 2, ['x']],
		'rebuild fields'
	);
	const r2 = parseReplayResponse('requeue_failed', requeue);
	assert(r2 && r2.mode === 'requeue_failed', 'requeue parsed');
	assertDeepEqual(
		[r2.requeued, r2.skippedNotFailed, r2.deliveryIds],
		[3, 4, ['y']],
		'requeue fields'
	);
	assertEqual(parseReplayResponse('rebuild', requeue), null, 'requeue body under rebuild');
	assertEqual(parseReplayResponse('requeue_failed', rebuild), null, 'rebuild body under requeue');
	assertEqual(parseReplayResponse('rebuild', { ...rebuild, requeued: 0 }), null, 'mixed fields');
	assertEqual(
		parseReplayResponse('rebuild', { replayed: 1, skipped_already_delivered: 2, window }),
		null,
		'missing ids'
	);
	assertEqual(
		parseReplayResponse('rebuild', { ...rebuild, window: undefined }),
		null,
		'missing window'
	);
});

await suite.checkAsync(
	'a replay response of the other mode is shown as malformed, not as a summary',
	async () => {
		const fake = fakeCommands({
			replay: [
				ok({
					requeued: 0,
					skipped_not_failed: 0,
					requeued_delivery_ids: [],
					window: { from: 'a', to: 'b' }
				})
			]
		});
		const replay = new DeliveryReplay(
			WS,
			{ commands: fake.commands, newKey: keys(), now: () => NOW },
			form()
		);
		const outcome = await replay.dryRun();
		assertEqual(outcome.status, 'failed', 'failed');
		assertEqual(replay.snapshot().preview, null, 'no summary');
		assertEqual(
			replay.snapshot().failure?.messageKey,
			'flow.operations.replay.error.malformed',
			'malformed key'
		);
	}
);

// ---- 6. health --------------------------------------------------------------------------------

await suite.check('delivery_cancelled is never dead-letter and never alerts', () => {
	const onlyCancelled = health({ delivery_cancelled: 9 });
	assertEqual(deadLetterTotal(onlyCancelled), 0, 'cancelled not counted');
	assertEqual(deadLetterAlert(onlyCancelled), false, 'no alert for cancellations');
	const failedOnes = health({
		dead_letter: { dispatch_failed: 1, delivery_failed: 2, oldest_failed_age: 30 },
		delivery_cancelled: 5
	});
	assertEqual(deadLetterTotal(failedOnes), 3, 'dispatch + delivery failed only');
	assertEqual(deadLetterAlert(failedOnes), true, 'alert when failed > 0');
});

await suite.checkAsync(
	'health refreshes on a 15s schedule, keeps stale data on failure, pauses, and stops on forbidden',
	async () => {
		const replies: Array<ApiResult<FlowAdminHealth>> = [
			ok(health({ connections: 2 })),
			err(500),
			ok(health({ connections: 5 })),
			err(403)
		];
		let calls = 0;
		const reads = {
			getAdminHealth: () => {
				calls += 1;
				const next = replies.shift();
				if (!next) throw new Error('unexpected health call');
				return Promise.resolve(next);
			}
		};
		const timers: Array<{ ms: number; cancelled: boolean; run: () => void }> = [];
		const schedule = (run: () => void, ms: number) => {
			const timer = { ms, cancelled: false, run };
			timers.push(timer);
			return () => {
				timer.cancelled = true;
			};
		};
		const seenSnapshots: HealthSnapshot[] = [];
		const monitor = new HealthMonitor(
			WS,
			reads,
			(snap) => seenSnapshots.push(snap),
			schedule,
			() => NOW
		);
		await monitor.refresh();
		monitor.start();
		assertEqual(timers[0].ms, 15_000, '15s period');
		const afterFail = await monitor.refresh();
		assertEqual(afterFail.health?.connections, 2, 'previous sample kept');
		assertEqual(afterFail.stale, true, 'marked stale');
		monitor.pause();
		assertEqual(timers[0].cancelled, true, 'pause cancels the timer');
		assertEqual(seenSnapshots.at(-1)?.paused, true, 'paused flag');
		monitor.resume();
		await new Promise((resolve) => setTimeout(resolve, 0));
		assertEqual(timers.length, 2, 'resume schedules again');
		assertEqual(monitor.snapshot().health?.connections, 5, 'resume refreshes');
		assertEqual(monitor.snapshot().stale, false, 'fresh sample clears stale');
		await monitor.refresh();
		assertEqual(monitor.snapshot().forbidden, true, 'forbidden');
		assertEqual(timers[1].cancelled, true, 'forbidden stops polling');
		assertEqual(calls, 4, 'four reads');
		monitor.dispose();
	}
);

await suite.checkAsync('page reads map 403/404 to the forbidden-safe state', async () => {
	const service = new FlowOperationsService(WS, {
		reads: {
			getAdminHealth: () => Promise.resolve(err(404)),
			getAdminLag: () => Promise.resolve(err(404)),
			getAdminIntegrity: () => Promise.resolve(err(403)),
			getProjectionLag: () => Promise.resolve(err(500))
		}
	});
	assertEqual((await service.loadLag()).status, 'forbidden', 'lag 404');
	assertEqual((await service.loadIntegrity(50)).status, 'forbidden', 'integrity 403');
	const lag = await service.projectionLagPager().load();
	assertEqual(lag.status, 'failed', 'projection-lag 500 is a failure, not forbidden');
});

await suite.checkAsync(
	'projection-lag pager forwards the server cursor and walks back',
	async () => {
		const seen: Array<string | undefined> = [];
		const pages: FlowProjectionLag[] = [
			{
				max_lag: 4,
				p95_lag: 1,
				items: [{ object_id: 'a', head_seq: 4, projection_seq: 0, lag: 4 }],
				next_cursor: 'c2'
			},
			{
				max_lag: 4,
				p95_lag: 1,
				items: [{ object_id: 'b', head_seq: 1, projection_seq: 1, lag: 0 }]
			},
			{
				max_lag: 4,
				p95_lag: 1,
				items: [{ object_id: 'a', head_seq: 4, projection_seq: 0, lag: 4 }],
				next_cursor: 'c2'
			}
		];
		const pager = new ProjectionLagPager(WS, {
			getProjectionLag: (_ws: string, query: { cursor?: string }) => {
				seen.push(query.cursor);
				return Promise.resolve(ok(pages.shift() as FlowProjectionLag));
			}
		});
		await pager.load();
		assertEqual(pager.hasPrevious(), false, 'first page');
		await pager.next();
		assertEqual(pager.hasNext(), false, 'no next cursor on page 2');
		assertEqual(pager.pageNumber, 2, 'page 2');
		await pager.previous();
		assertDeepEqual(seen, [undefined, 'c2', undefined], 'cursor sequence');
		assertEqual(pager.pageNumber, 1, 'back on page 1');
	}
);

await suite.checkAsync(
	'projection-lag pager: previous from page 3 returns to page 2 with page 2 cursor',
	async () => {
		// Four pages keyed by cursor, so a request for the wrong cursor returns the wrong page.
		const byCursor: Record<string, FlowProjectionLag> = {
			'': { max_lag: 9, p95_lag: 1, items: [lagItem('p1')], next_cursor: 'c2' },
			c2: { max_lag: 9, p95_lag: 1, items: [lagItem('p2')], next_cursor: 'c3' },
			c3: { max_lag: 9, p95_lag: 1, items: [lagItem('p3')], next_cursor: 'c4' },
			c4: { max_lag: 9, p95_lag: 1, items: [lagItem('p4')] }
		};
		const seen: Array<string | undefined> = [];
		const pager = new ProjectionLagPager(WS, {
			getProjectionLag: (_ws: string, query: { cursor?: string }) => {
				seen.push(query.cursor);
				return Promise.resolve(ok(byCursor[query.cursor ?? '']));
			}
		});
		const firstItem = () => pager.current?.items[0]?.object_id;
		await pager.load();
		await pager.next();
		await pager.next();
		assertEqual(pager.pageNumber, 3, 'on page 3');
		assertEqual(firstItem(), 'p3', 'page 3 rows');
		await pager.previous();
		assertEqual(pager.pageNumber, 2, 'previous from 3 is page 2, not page 1');
		assertEqual(firstItem(), 'p2', 'page 2 rows');
		assertEqual(seen[seen.length - 1], 'c2', 'page 2 is fetched with its own cursor');
		assertEqual(pager.hasPrevious(), true, 'page 1 is still behind');
		await pager.next();
		await pager.next();
		assertEqual(pager.pageNumber, 4, 'forward again to page 4');
		assertEqual(pager.hasNext(), false, 'page 4 is last');
		await pager.previous();
		await pager.previous();
		assertEqual(pager.pageNumber, 2, 'two steps back from 4 is 2');
		assertEqual(firstItem(), 'p2', 'page 2 rows again');
		await pager.previous();
		assertEqual(pager.pageNumber, 1, 'then page 1');
		assertEqual(firstItem(), 'p1', 'page 1 rows');
		assertDeepEqual(
			seen,
			[undefined, 'c2', 'c3', 'c2', 'c3', 'c4', 'c3', 'c2', undefined],
			'cursor sequence'
		);
	}
);

await suite.checkAsync(
	'replay execute re-checks the window at execute time: a dry-run that aged out is refused locally',
	async () => {
		const fake = fakeCommands({
			replay: [
				ok({
					replayed: 0,
					skipped_already_delivered: 0,
					rebuilt_delivery_ids: [],
					window: { from: 'a', to: 'b' }
				})
			]
		});
		let clock = NOW;
		const replay = new DeliveryReplay(
			WS,
			{ commands: fake.commands, newKey: keys(), now: () => clock },
			// 29 days and 23 hours ago: inside the window at dry-run time.
			form({ from: new Date(NOW - 30 * DAY + HOUR).toISOString() })
		);
		assertEqual((await replay.dryRun()).status, 'ok', 'dry-run inside the window');
		replay.acknowledge(true);
		assertEqual(replay.canExecute(), true, 'executable right after the dry-run');
		// The panel stays open for two hours: the same window now starts more than 30 days ago.
		clock = NOW + 2 * HOUR;
		assertEqual(replay.canExecute(), false, 'window aged out -> not executable');
		assertEqual(replay.snapshot().canExecute, false, 'snapshot agrees');
		assertEqual((await replay.execute()).status, 'refused', 'execute refused locally');
		assertEqual(fake.calls.length, 1, 'only the dry-run was sent; no execute request');
	}
);

await suite.checkAsync('every static i18n key used by the panel exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	const required = [
		...Object.values(HEALTH_STATUS_KEYS),
		...Object.values(INTEGRITY_STATUS_KEYS),
		...Object.values(MAINTENANCE_TITLE_KEYS),
		...Object.values(OPERATION_NAME_KEYS),
		...Object.values(RECEIPT_FIELD_KEYS),
		...Object.values(REPLAY_MODE_KEYS),
		'flow.operations.status.unknown',
		'flow.operations.lag.projection',
		'flow.operations.lag.search',
		'flow.operations.lag.fanout',
		'flow.operations.error.notFound',
		'flow.operations.error.conflict',
		'flow.operations.error.rejected',
		'flow.operations.error.unavailable',
		'flow.operations.error.unexpectedResponse',
		'flow.operations.replay.error.invalidDate',
		'flow.operations.replay.error.empty',
		'flow.operations.replay.error.future',
		'flow.operations.replay.error.tooOld',
		'flow.operations.replay.error.invalidSubscriber',
		'flow.operations.replay.error.malformed',
		'flow.error.server_rejected',
		'flow.error.forbidden',
		'flow.error.unauthenticated'
	];
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		const missing = required.filter((key) => typeof lookup(key) !== 'string');
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
		assertEqual(
			typeof lookup('pageTitle.flowOperations'),
			'string',
			`${locale}.json pageTitle.flowOperations`
		);
	}
});

finish(suite.result());
