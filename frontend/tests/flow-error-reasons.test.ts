/**
 * vF `invalid_update_named_reason_consumers_v0_5` and `server_draining_ui_consumer_v0_4`
 * (`gates/vF-frontend-gate.yaml`):
 *
 *   - 生产 TypeScript 存在具名 invalid-update reason 的消费者（非仅类型声明）
 *   - 未知 reason 走统一 fallback，不按 message 文本分支
 *   - defensive reason 进入 integrity repair 路径
 *   - UI 对每个 server_draining reason 都有对应消费行为（含 contention）
 *
 * Behaviour, not declarations: the dispositions are read off the real envelope/frame decoders; the
 * `idempotency_key_body_drift` consumer is the real `CommandService` surfacing it as a typed error
 * (never resending under a new key, `error-mapping-v1.md` "drift 不自动换 key"); the
 * production call sites are located in the shipped source (comments stripped), so deleting a
 * consumer turns this red even if the helper stays.
 *
 * Run standalone: `bun tests/flow-error-reasons.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertEqual, assertNotEqual, finish } from './support/harness';
import {
	FLOW_ERROR_CODES,
	FLOW_LOCAL_ERROR_CODES,
	INVALID_UPDATE_REASONS,
	drainDisposition,
	flowErrorDetailKey,
	flowErrorFromEnvelope,
	flowErrorFromRejectedFrame,
	flowErrorI18nKey,
	invalidUpdateDisposition
} from '../src/lib/flow/errors';
import {
	FlowCommandService,
	flowErrorFromResult,
	mapErrorCode
} from '../src/lib/flow/command-service';
import type { ApiResult } from '../src/lib/api/client';

const suite = new Suite('flow_error_reasons');
const ROOT = new URL('..', import.meta.url).pathname;

function envelope(reason: string | undefined, message = 'subtree_spans_multiple_projects') {
	return {
		code: 400,
		message,
		data: null,
		error_code: 'invalid_update',
		details: reason === undefined ? undefined : { reason }
	};
}

suite.check('each named reason maps to its action and key, on REST and WS alike', () => {
	const expected: Record<string, string> = {
		subtree_spans_multiple_projects: 'integrity_repair',
		child_project_must_match_parent: 'change_project',
		project_unmapped: 'remap_project',
		idempotency_key_body_drift: 'surface_error',
		invalid_field_value: 'fix_field_value',
		causal_dependency_missing: 'keep_in_recovery_draft',
		cross_workspace_relation: 'integrity_repair'
	};
	// The table above is the independent expectation: the decoder must know exactly these
	// reasons, so dropping one from `INVALID_UPDATE_REASONS` is red, not silently skipped.
	assertEqual(
		JSON.stringify([...INVALID_UPDATE_REASONS].sort()),
		JSON.stringify(Object.keys(expected).sort()),
		'the client knows exactly the approved reasons'
	);
	for (const reason of Object.keys(expected)) {
		const rest = flowErrorFromEnvelope(envelope(reason));
		const ws = flowErrorFromRejectedFrame({
			type: 'rejected',
			code: 'invalid_update',
			recoverable: false,
			details: { reason }
		});
		for (const [surface, error] of [
			['REST', rest],
			['WS', ws]
		] as const) {
			assert(error, `${surface} decodes`);
			const disposition = invalidUpdateDisposition(error);
			assertEqual(disposition?.action, expected[reason], `${surface} ${reason} action`);
			assertEqual(
				flowErrorDetailKey(error),
				`flow.error.invalid_update_reason.${reason}`,
				`${surface} ${reason} key`
			);
			assertEqual(error.recoverable, false, `${surface} ${reason} stays non-recoverable`);
		}
	}
});

suite.check(
	'network_error is Web-local: a 5xx without error_code, recoverable, never a server code',
	() => {
		assert(
			!(FLOW_ERROR_CODES as readonly string[]).includes('network_error'),
			'network_error is not in the server code set'
		);
		assert(
			(FLOW_LOCAL_ERROR_CODES as readonly string[]).includes('network_error'),
			'network_error is a local code'
		);
		for (const code of [500, 502, 503, 504]) {
			const error = mapErrorCode(code);
			assertEqual(error.code, 'network_error', `${code} -> network_error`);
			assertEqual(error.recoverable, true, `${code} recoverable`);
			assertEqual(error.origin, 'client', `${code} is minted locally`);
			assertEqual(flowErrorI18nKey(error), 'flow.error.network_error', `${code} key`);
		}
		assertEqual(mapErrorCode(422).code, 'invalid_update', 'unknown 4xx stays invalid_update');
		assertEqual(mapErrorCode(422).recoverable, false, 'and non-recoverable');
		assertEqual(
			flowErrorFromResult({ code: 500, error_code: 'server_rejected' }).code,
			'server_rejected',
			'a typed 5xx keeps its code'
		);
		// A peer that names a local code on the wire is not decoded as one.
		assertEqual(
			flowErrorFromEnvelope({ code: 500, error_code: 'network_error' }),
			null,
			'the envelope decoder does not accept network_error'
		);
		assertEqual(
			flowErrorFromResult({ code: 400, error_code: 'network_error' }).code,
			'invalid_update',
			'a 4xx naming network_error is still a server refusal'
		);
	}
);

suite.check('the defensive reason is the integrity-repair path', () => {
	const error = flowErrorFromEnvelope(envelope('subtree_spans_multiple_projects'));
	assert(error, 'decodes');
	assertEqual(invalidUpdateDisposition(error)?.action, 'integrity_repair', 'integrity repair');
});

suite.check('unknown or absent reason: the unified fallback, never the message', () => {
	for (const reason of ['not_a_reason', '', undefined]) {
		// The message names a known reason; it must not be read.
		const error = flowErrorFromEnvelope(envelope(reason, 'subtree_spans_multiple_projects'));
		assert(error, 'decodes');
		assertEqual(
			invalidUpdateDisposition(error),
			null,
			`reason=${String(reason)} has no disposition`
		);
		assertEqual(flowErrorDetailKey(error), 'flow.error.invalid_update', 'generic key');
		assertEqual(error.code, 'invalid_update', 'classification unchanged');
		assertEqual(error.recoverable, false, 'still non-recoverable');
	}
	const other = flowErrorFromEnvelope({
		code: 403,
		error_code: 'policy_rejected',
		details: { reason: 'subtree_spans_multiple_projects' }
	});
	assert(other, 'decodes');
	assertEqual(
		invalidUpdateDisposition(other),
		null,
		'a reason on another code is not an invalid_update reason'
	);
});

const DRIFT = {
	code: 400,
	message: 'irrelevant prose',
	data: null,
	error_code: 'invalid_update',
	details: { reason: 'idempotency_key_body_drift' }
};

/** Runs `body` against a `fetch` that answers every request with `answer`; returns the bodies. */
async function withFetch(
	answer: unknown,
	body: () => Promise<void>
): Promise<Array<Record<string, unknown>>> {
	const seen: Array<Record<string, unknown>> = [];
	const original = globalThis.fetch;
	globalThis.fetch = (async (_input: string | URL | Request, init?: RequestInit) => {
		seen.push(init?.body ? JSON.parse(String(init.body)) : {});
		return new Response(JSON.stringify(answer), {
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	try {
		await body();
	} finally {
		globalThis.fetch = original;
	}
	return seen;
}

await suite.checkAsync(
	'idempotency_key_body_drift: thrown as a typed error, never resent under a new key',
	async () => {
		const commands = new FlowCommandService();
		const writes: Array<[string, () => Promise<unknown>]> = [
			[
				'createObject',
				() =>
					commands.createObject(
						'ws-1',
						{ object_type: 'page', title: 'x' },
						{ idempotencyKey: 'k-create' }
					)
			],
			[
				'execute',
				() =>
					commands.execute(
						'obj-1',
						{ type: 'set_title', payload: { title: 'x' } },
						{ idempotencyKey: 'k-execute', expectedFrontier: 'f1' }
					)
			],
			[
				'createRecord',
				() => commands.createRecord('col-1', { values_by_field_id: {} }, { idempotencyKey: 'k-record' })
			]
		];
		for (const [method, send] of writes) {
			let thrown: unknown = null;
			const bodies = await withFetch(DRIFT, async () => {
				try {
					await send();
				} catch (error) {
					thrown = error;
				}
			});
			assertEqual(bodies.length, 1, `${method}: sent once, no automatic resend`);
			const error = thrown as { code?: string; details?: { reason?: string } } | null;
			assertEqual(error?.code, 'invalid_update', `${method}: typed code`);
			assertEqual(
				error?.details?.reason,
				'idempotency_key_body_drift',
				`${method}: the drift reason reaches the caller`
			);
			assertEqual(
				invalidUpdateDisposition(error as never)?.action,
				'surface_error',
				`${method}: shown as an error, not re-keyed`
			);
		}
	}
);

suite.check(
	'server_draining: both reasons have a distinct consumer behaviour; unknown fails closed',
	() => {
		const drain = drainDisposition({
			code: 'server_draining',
			details: { reason: 'drain', retry_after_ms: 10 }
		});
		const contention = drainDisposition({
			code: 'server_draining',
			details: { reason: 'contention' }
		});
		assert(drain && contention, 'both dispositions exist');
		assertEqual(drain.keepConnection, false, 'drain gives the connection up');
		assertEqual(contention.keepConnection, true, 'contention keeps it');
		assertNotEqual(drain.i18nKey, contention.i18nKey, 'different keys');
		assertEqual(
			drainDisposition({ code: 'server_draining', details: { reason: 'maintenance' } }),
			null,
			'unknown reason: no guess'
		);
		assertEqual(
			flowErrorI18nKey({ code: 'server_draining', details: {} }),
			'flow.error.unsupported_protocol',
			'unified fallback for a missing discriminator'
		);
	}
);

suite.check('network_error copy says the result is unknown, never that nothing was committed', () => {
	for (const [locale, unknown, claims] of [
		['en', /unknown/i, /nothing was committed|not (been )?committed|was not saved/i],
		['zh', /未知/, /尚未提交|未提交|没有提交/]
	] as const) {
		const data = JSON.parse(readFileSync(join(ROOT, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const text: unknown = data?.flow?.error?.network_error;
		assert(typeof text === 'string', `${locale}: flow.error.network_error exists`);
		assert(unknown.test(text as string), `${locale}: says the outcome is unknown: ${text}`);
		assert(!claims.test(text as string), `${locale}: does not claim nothing was committed: ${text}`);
	}
});

// ---- the production consumers exist (not just the helper) ---------------------------------------

function code(path: string): string {
	const source = readFileSync(join(ROOT, path), 'utf8');
	return source
		.replace(/<!--[\s\S]*?-->/g, '')
		.replace(/\/\*[\s\S]*?\*\//g, '')
		.replace(/(^|[^:])\/\/[^\n]*/g, '$1');
}

suite.check('production call sites consume the named reasons', () => {
	const route = code('src/routes/(app)/workspace/[workspaceId]/flow/[objectId]/+page.svelte');
	assert(
		/invalidUpdateDisposition\(runtimeError\)\?\.action === 'integrity_repair'/.test(route),
		'object page: integrity repair branch'
	);
	assert(/settings\/flow\/operations/.test(route), 'object page: links to the operations page');
	assert(/flowErrorDetailKey\(runtimeError\)/.test(route), 'object page: reason-specific copy');
	const navigator = code('src/lib/components/flow/FlowNavigator.svelte');
	assert(/flowErrorDetailKey\(/.test(navigator), 'navigator create: reason-specific copy');
	const commands = code('src/lib/flow/command-service.ts');
	assert(!/rekey|Rekey|mintKey/.test(commands), 'CommandService has no automatic re-key path');
	const session = code('src/lib/flow/object-session.ts');
	assert(/drainDisposition\(error\)/.test(session), 'the session consumes server_draining reasons');
});

export const result = suite.result();

if (import.meta.main) finish(result);
