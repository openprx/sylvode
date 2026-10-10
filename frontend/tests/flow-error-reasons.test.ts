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
 * `idempotency_key_body_drift` consumer is the real `CommandService` resending under a new key; the
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
	flowErrorFromResult,
	mapErrorCode,
	sendWithDriftRekey
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
		idempotency_key_body_drift: 'rekey_and_resend',
		invalid_field_value: 'fix_field_value',
		causal_dependency_missing: 'keep_in_recovery_draft',
		cross_workspace_relation: 'integrity_repair'
	};
	for (const reason of INVALID_UPDATE_REASONS) {
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

await suite.checkAsync('idempotency_key_body_drift: resent once under a NEW key', async () => {
	const keys: string[] = [];
	const answers: Array<ApiResult<string>> = [
		{
			code: 400,
			message: 'x',
			data: null,
			error_code: 'invalid_update',
			details: { reason: 'idempotency_key_body_drift' }
		} as ApiResult<string>,
		{ code: 0, message: 'ok', data: 'done' }
	];
	const result = await sendWithDriftRekey(
		async (key) => {
			keys.push(key);
			return answers[keys.length - 1];
		},
		'original-key',
		() => 'fresh-key'
	);
	assertEqual(result.code, 0, 'the resend succeeded');
	assertEqual(keys.length, 2, 'sent twice');
	assertEqual(keys[0], 'original-key', 'first under the caller key');
	assertNotEqual(keys[1], keys[0], 'the resend uses a new key');
});

await suite.checkAsync(
	'any other refusal is returned as is, and a second drift is not chased',
	async () => {
		for (const details of [
			{ reason: 'child_project_must_match_parent' },
			{ reason: 'other' },
			undefined
		]) {
			let calls = 0;
			await sendWithDriftRekey(async () => {
				calls += 1;
				return {
					code: 400,
					message: '',
					data: null,
					error_code: 'invalid_update',
					details
				} as ApiResult<string>;
			}, 'k');
			assertEqual(calls, 1, `no resend for ${JSON.stringify(details)}`);
		}
		let calls = 0;
		const drift = {
			code: 400,
			message: '',
			data: null,
			error_code: 'invalid_update',
			details: { reason: 'idempotency_key_body_drift' }
		} as ApiResult<string>;
		const result = await sendWithDriftRekey(async () => {
			calls += 1;
			return drift;
		}, 'k');
		assertEqual(calls, 2, 'one resend, then the second drift is returned');
		assertEqual(result.error_code, 'invalid_update', 'returned to the caller');
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
	for (const method of ['createObject', 'execute', 'createRecord']) {
		const body = commands.slice(commands.indexOf(`async ${method}(`));
		const end = body.indexOf('\n\t}\n');
		assert(
			/sendWithDriftRekey\(/.test(body.slice(0, end)),
			`CommandService.${method} re-keys on drift`
		);
	}
	const session = code('src/lib/flow/object-session.ts');
	assert(/drainDisposition\(error\)/.test(session), 'the session consumes server_draining reasons');
});

export const result = suite.result();

if (import.meta.main) finish(result);
