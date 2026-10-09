/**
 * Workspace Flow settings (`/workspace/{id}/settings/flow`) against `GET|PUT
 * /workspaces/{workspace_id}/features/flow` (`contracts/rest-api-v1.md` v0.4 table).
 *
 * Drives the real `FlowSettingsController` -> `FlowCommandService` -> `flowApi` -> `apiClient`
 * chain with `globalThis.fetch` mocked, so every assertion is about the bytes that would leave
 * the browser: request shape per field, idempotency-key reuse on retry, the confirmation gate in
 * front of the two dangerous changes, `error_code`-only error branching, and the fail-closed
 * sidebar flag store.
 *
 * Run standalone: `bun tests/flow-settings.test.ts`
 */

import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	assertNotEqual,
	finish
} from './support/harness';
import {
	FlowSettingsChangeError,
	FlowSettingsController,
	buildFlowFeatureRequest,
	classifyFlowSettingsError,
	type FlowSettingsChange
} from '../src/lib/flow/flow-settings';
import { createFlowFeatureStore, flowEnabledFromResult } from '../src/lib/stores/flow-feature';
import { FlowCommandService } from '../src/lib/flow/command-service';
import type { ApiResult } from '../src/lib/api/client';
import type { FlowFeatureFlags } from '../src/lib/api/flow';

interface Call {
	method: string;
	path: string;
	body: unknown;
}

type Reply = {
	code: number;
	message?: string;
	data?: unknown;
	error_code?: string;
	details?: unknown;
};

const WORKSPACE = '11111111-1111-4111-8111-111111111111';
const FEATURE_PATH = `/api/v1/workspaces/${WORKSPACE}/features/flow`;

let calls: Call[] = [];
let replies: Reply[] = [];
let state: FlowFeatureFlags;

function resetServer(initial: Partial<FlowFeatureFlags> = {}): void {
	calls = [];
	replies = [];
	state = {
		flow_enabled: true,
		default_member_level: 'edit',
		authz_epoch: 7,
		updated_at: '2026-10-09T00:00:00Z',
		updated_by: '22222222-2222-4222-8222-222222222222',
		...initial
	};
}

globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const path = raw.startsWith('http') ? new URL(raw).pathname : raw;
	const method = init?.method ?? 'GET';
	const body = init?.body ? JSON.parse(String(init.body)) : null;
	calls.push({ method, path, body });
	let reply: Reply;
	const queued = replies.shift();
	if (queued) {
		reply = queued;
	} else if (method === 'PUT') {
		const req = body as {
			enabled?: boolean;
			default_member_level?: FlowFeatureFlags['default_member_level'];
		};
		if (req.enabled !== undefined) state.flow_enabled = req.enabled;
		if (req.default_member_level !== undefined)
			state.default_member_level = req.default_member_level;
		state.authz_epoch += 1;
		reply = { code: 0, message: 'ok', data: { ...state, event_id: `event-${calls.length}` } };
	} else {
		reply = { code: 0, message: 'ok', data: { ...state } };
	}
	return new Response(JSON.stringify({ message: 'ok', data: null, ...reply }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

function controller(admin = true) {
	const store = createFlowFeatureStore((id) => new FlowCommandService().getFlowFeature(id));
	const ctl = new FlowSettingsController(WORKSPACE, {
		store,
		isWorkspaceAdmin: async () => admin
	});
	return { ctl, store };
}

function puts(): Call[] {
	return calls.filter((call) => call.method === 'PUT');
}

const suite = new Suite('flow-settings');

// ---- 1. request shape -------------------------------------------------------------------

await suite.checkAsync(
	'GET reads the workspace feature endpoint and fills the status card',
	async () => {
		resetServer();
		const { ctl, store } = controller();
		const outcome = await ctl.load();
		assertEqual(outcome.status, 'ready', 'load outcome');
		assertDeepEqual(calls, [{ method: 'GET', path: FEATURE_PATH, body: null }], 'one GET, no body');
		assertEqual(ctl.current?.authz_epoch, 7, 'authz_epoch read through');
		assertEqual(store.enabled(WORKSPACE), true, 'load records the flag in the shared store');
	}
);

await suite.checkAsync('enable PUT carries only `enabled` and the intent key', async () => {
	resetServer({ flow_enabled: false });
	const { ctl } = controller();
	await ctl.load();
	const intent = ctl.propose({ field: 'enabled', value: true });
	assert(intent, 'enabling a disabled workspace is a change');
	assertEqual(intent.requiresConfirmation, false, 'enable needs no confirmation');
	const outcome = await ctl.submit(intent);
	assertEqual(outcome.status, 'saved', 'saved');
	const [put] = puts();
	assertEqual(put.path, FEATURE_PATH, 'PUT path');
	assertDeepEqual(
		Object.keys(put.body as object).sort(),
		['enabled', 'idempotency_key'],
		'body holds exactly the changed field plus the key'
	);
	assertEqual((put.body as { enabled: boolean }).enabled, true, 'enabled value');
	assertEqual(
		(put.body as { idempotency_key: string }).idempotency_key,
		intent.idempotencyKey,
		'key is the intent key'
	);
	if (outcome.status === 'saved') {
		assertEqual(outcome.eventId, `event-${calls.length}`, 'event_id surfaced for the toast');
		assertEqual(outcome.feature.authz_epoch, 8, 'status card refreshed from the response');
	}
});

await suite.checkAsync(
	'level PUT carries only `default_member_level` and the intent key',
	async () => {
		resetServer();
		const { ctl } = controller();
		await ctl.load();
		const intent = ctl.propose({ field: 'default_member_level', value: 'view' });
		assert(intent, 'edit -> view is a change');
		ctl.confirm(intent);
		await ctl.submit(intent);
		const [put] = puts();
		assertDeepEqual(
			put.body,
			{ default_member_level: 'view', idempotency_key: intent.idempotencyKey },
			'level body'
		);
	}
);

await suite.checkAsync('the two fields are separate intents with separate keys', async () => {
	resetServer();
	const { ctl } = controller();
	await ctl.load();
	const level = ctl.propose({ field: 'default_member_level', value: 'comment' });
	assert(level, 'level intent');
	ctl.cancel();
	const disable = ctl.propose({ field: 'enabled', value: false });
	assert(disable, 'disable intent');
	assertNotEqual(level.idempotencyKey, disable.idempotencyKey, 'each intent has its own key');
});

// ---- 2. empty / invalid requests are refused client-side ---------------------------------

await suite.checkAsync('an empty change is refused before any request', async () => {
	resetServer();
	const { ctl } = controller();
	await ctl.load();
	const before = calls.length;
	for (const empty of [null, undefined]) {
		let refused = false;
		try {
			ctl.propose(empty);
		} catch (error) {
			refused = error instanceof FlowSettingsChangeError && error.reason === 'empty_change';
		}
		assert(refused, `propose(${String(empty)}) must throw empty_change`);
		let builderRefused = false;
		try {
			buildFlowFeatureRequest(empty, 'k');
		} catch (error) {
			builderRefused = error instanceof FlowSettingsChangeError && error.reason === 'empty_change';
		}
		assert(builderRefused, `buildFlowFeatureRequest(${String(empty)}) must throw empty_change`);
	}
	let invalid = false;
	try {
		ctl.propose({ field: 'default_member_level', value: 'owner' } as unknown as FlowSettingsChange);
	} catch (error) {
		invalid = error instanceof FlowSettingsChangeError && error.reason === 'invalid_value';
	}
	assert(invalid, 'a level outside the contract set is refused');
	assertEqual(calls.length, before, 'no request was made for any refused change');
});

await suite.checkAsync(
	'a change equal to the current value produces no intent and no request',
	async () => {
		resetServer();
		const { ctl } = controller();
		await ctl.load();
		const before = calls.length;
		assertEqual(ctl.propose({ field: 'enabled', value: true }), null, 'already enabled');
		assertEqual(
			ctl.propose({ field: 'default_member_level', value: 'edit' }),
			null,
			'already edit'
		);
		assertEqual(calls.length, before, 'no request');
	}
);

// ---- 3. dangerous changes are gated on an explicit confirmation --------------------------

await suite.checkAsync(
	'disable is not sent until the user confirms that exact intent',
	async () => {
		resetServer();
		const { ctl } = controller();
		await ctl.load();
		const intent = ctl.propose({ field: 'enabled', value: false });
		assert(intent, 'disable intent');
		assertEqual(intent.requiresConfirmation, true, 'disable requires confirmation');
		const unconfirmed = await ctl.submit(intent);
		assertEqual(unconfirmed.status, 'confirmation_required', 'unconfirmed submit is refused');
		assertEqual(puts().length, 0, 'no PUT before confirmation');
		ctl.confirm(intent);
		const confirmed = await ctl.submit(intent);
		assertEqual(confirmed.status, 'saved', 'confirmed submit goes out');
		assertEqual(puts().length, 1, 'exactly one PUT');
		assertDeepEqual(
			puts()[0].body,
			{ enabled: false, idempotency_key: intent.idempotencyKey },
			'disable body'
		);
	}
);

await suite.checkAsync(
	'a level change is not sent until confirmed, and a confirmation does not carry over',
	async () => {
		resetServer();
		const { ctl } = controller();
		await ctl.load();
		const first = ctl.propose({ field: 'default_member_level', value: 'view' });
		assert(first, 'first intent');
		ctl.confirm(first);
		// The user changes their mind before submitting: the new intent must be confirmed afresh.
		const second = ctl.propose({ field: 'default_member_level', value: 'comment' });
		assert(second, 'second intent');
		const outcome = await ctl.submit(second);
		assertEqual(
			outcome.status,
			'confirmation_required',
			'old confirmation does not cover the new intent'
		);
		assertEqual(puts().length, 0, 'no PUT');
		let stale = false;
		try {
			ctl.confirm(first);
		} catch (error) {
			stale = error instanceof FlowSettingsChangeError && error.reason === 'stale_intent';
		}
		assert(stale, 'confirming a superseded intent is refused');
	}
);

// ---- 4. idempotency key reuse on retry --------------------------------------------------

await suite.checkAsync('a retry of the same intent resends the same key', async () => {
	resetServer();
	const { ctl } = controller();
	await ctl.load();
	const intent = ctl.propose({ field: 'enabled', value: false });
	assert(intent, 'intent');
	ctl.confirm(intent);
	replies.push({ code: 500, message: 'database error' });
	const failed = await ctl.submit(intent);
	assertEqual(failed.status, 'failed', 'first attempt fails');
	if (failed.status === 'failed') assertEqual(failed.retryable, true, '5xx is retryable');
	assertEqual(ctl.pendingIntent, intent, 'the intent survives a retryable failure');
	const again = ctl.propose({ field: 'enabled', value: false });
	assertEqual(again, intent, 're-proposing the same change resumes the same intent');
	const saved = await ctl.submit(intent);
	assertEqual(saved.status, 'saved', 'retry succeeds');
	const [firstPut, secondPut] = puts();
	assertEqual(
		(secondPut.body as { idempotency_key: string }).idempotency_key,
		(firstPut.body as { idempotency_key: string }).idempotency_key,
		'retry key equals original key'
	);
	const next = ctl.propose({ field: 'enabled', value: true });
	assert(next, 'a later, different intent');
	assertNotEqual(next.idempotencyKey, intent.idempotencyKey, 'a new intent mints a new key');
});

// ---- 5. error branching reads error_code, never message ---------------------------------

await suite.checkAsync(
	'error_code=forbidden lands on the forbidden state whatever the message says',
	async () => {
		resetServer();
		const { ctl } = controller();
		await ctl.load();
		const intent = ctl.propose({ field: 'enabled', value: false });
		assert(intent, 'intent');
		ctl.confirm(intent);
		replies.push({ code: 403, message: 'everything is fine', error_code: 'forbidden' });
		const outcome = await ctl.submit(intent);
		assertEqual(outcome.status, 'forbidden', 'forbidden by error_code');
		assertEqual(ctl.pendingIntent, null, 'forbidden ends the intent (no retry offered)');
	}
);

suite.check('a message that says "forbidden" does not make an error forbidden', () => {
	const byMessage = classifyFlowSettingsError({
		code: 400,
		message: 'forbidden',
		error_code: undefined
	} as ApiResult<unknown>);
	assertEqual(byMessage.kind, 'failed', 'message is ignored');
	const typedWins = classifyFlowSettingsError({ code: 403, error_code: 'invalid_update' });
	assertDeepEqual(
		typedWins,
		{ kind: 'failed', messageKey: 'flow.error.invalid_update', retryable: false },
		'error_code wins over the numeric code'
	);
	const legacyForbidden = classifyFlowSettingsError({ code: 403 });
	assertEqual(
		legacyForbidden.kind,
		'forbidden',
		'legacy Forbidden/403 envelope without error_code'
	);
	const legacyNotFound = classifyFlowSettingsError({ code: 404 });
	assertEqual(legacyNotFound.kind, 'forbidden', 'not_found does not reveal existence');
	const unknownTyped = classifyFlowSettingsError({ code: 500, error_code: 'something_new' });
	assertDeepEqual(
		unknownTyped,
		{ kind: 'failed', messageKey: 'flow.settings.error.rejected', retryable: false },
		'unknown typed code fails closed without retry'
	);
	const draining = classifyFlowSettingsError({
		code: 503,
		error_code: 'server_draining',
		details: { reason: 'contention' }
	});
	assertDeepEqual(
		draining,
		{ kind: 'failed', messageKey: 'flow.error.server_draining.contention', retryable: true },
		'server_draining keyed by details.reason'
	);
});

await suite.checkAsync(
	'a non-admin never loads the form and never reads the endpoint',
	async () => {
		resetServer();
		const { ctl } = controller(false);
		const outcome = await ctl.load();
		assertEqual(outcome.status, 'forbidden', 'non-admin -> forbidden-safe state');
		assertEqual(calls.length, 0, 'no request made');
	}
);

await suite.checkAsync(
	'a GET answered with error_code=forbidden lands on the forbidden state',
	async () => {
		resetServer();
		replies.push({ code: 403, message: 'ok', error_code: 'forbidden' });
		const { ctl, store } = controller();
		const outcome = await ctl.load();
		assertEqual(outcome.status, 'forbidden', 'forbidden');
		assertEqual(store.enabled(WORKSPACE), false, 'store untouched (still false)');
	}
);

// ---- 6. store fails closed --------------------------------------------------------------

await suite.checkAsync('the flag store reads every non-success result as false', async () => {
	const results: Array<ApiResult<FlowFeatureFlags> | Error> = [];
	const store = createFlowFeatureStore(async () => {
		const next = results.shift();
		if (next instanceof Error) throw next;
		if (!next) throw new Error('no fixture');
		return next;
	});
	const enabled: ApiResult<FlowFeatureFlags> = {
		code: 0,
		message: 'ok',
		data: { ...state, flow_enabled: true }
	};
	results.push(enabled);
	assertEqual(await store.refresh(WORKSPACE), true, 'success + flow_enabled=true');
	const failures: Array<ApiResult<FlowFeatureFlags> | Error> = [
		{ code: 403, message: 'ok', data: null, error_code: 'forbidden' },
		{ code: 404, message: 'ok', data: null },
		{ code: 500, message: 'Network error', data: null },
		{ code: 0, message: 'ok', data: null },
		new Error('fetch rejected')
	];
	for (const failure of failures) {
		results.push(enabled);
		await store.refresh(WORKSPACE);
		results.push(failure);
		assertEqual(
			await store.refresh(WORKSPACE),
			false,
			`non-success ${failure instanceof Error ? failure.message : JSON.stringify(failure)} reads false`
		);
	}
	assertEqual(store.enabled('never-read'), false, 'an unread workspace is false');
	assertEqual(flowEnabledFromResult(null), false, 'null result is false');
	assertEqual(
		flowEnabledFromResult({ code: 7, message: 'ok', data: { ...state, flow_enabled: true } }),
		false,
		'non-zero code with a data body is still false'
	);
});

await suite.checkAsync('a saved disable flips the shared store the sidebar reads', async () => {
	resetServer();
	const { ctl, store } = controller();
	await ctl.load();
	assertEqual(store.enabled(WORKSPACE), true, 'enabled before');
	const intent = ctl.propose({ field: 'enabled', value: false });
	assert(intent, 'intent');
	ctl.confirm(intent);
	await ctl.submit(intent);
	assertEqual(store.enabled(WORKSPACE), false, 'store follows the PUT response');
});

await suite.checkAsync(
	'a slow stale refresh cannot overwrite a value recorded after it started',
	async () => {
		let release: (value: ApiResult<FlowFeatureFlags>) => void = () => undefined;
		const store = createFlowFeatureStore(
			() => new Promise<ApiResult<FlowFeatureFlags>>((resolve) => (release = resolve))
		);
		const pending = store.refresh(WORKSPACE);
		store.set(WORKSPACE, false);
		release({ code: 0, message: 'ok', data: { ...state, flow_enabled: true } });
		await pending;
		assertEqual(store.enabled(WORKSPACE), false, 'the later set() wins');
	}
);

export const result = suite.result();

if (import.meta.main) finish(result);
