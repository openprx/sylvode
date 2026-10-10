// The ONE place the Flow UI turns a wire rejection into (a) an i18n key and (b) a retry
// disposition. Every surface that can carry a Flow business error -- the REST envelope
// (`apps/api/src/error.rs::IntoResponse for ApiError`), a WebSocket `rejected` control frame
// (`apps/api/src/flow/collab/frame.rs::Frame::Rejected`) and a WebSocket close
// (`apps/api/src/flow/collab/registry.rs`'s 4410 drain close) -- funnels through here, so the
// same producer fixture cannot end up branching differently depending on which transport
// delivered it.
//
// `contracts/error-mapping-v1.md` "Wire rules": "UI 使用稳定 code 与该 code 冻结的 typed
// discriminator 选择 i18n key，禁止用英文 message 分支。`server_draining` 唯一 discriminator 是
// required `details.reason`." Nothing in this module ever reads `message`.

import type { FlowError, FlowErrorCode, SyncState } from './types';

/** Every stable error code this client can actually receive, as a runtime value.
 *
 * `contracts/error-mapping-v1.md`'s "稳定错误的五层映射" table has fourteen rows. Two of them
 * (`checksum_mismatch`, `unsupported_format`) are marked "n/a；import/export REST only": their
 * only producer is the v0.8 package round-trip surface (`contracts/export-package-v1.md`). That
 * surface is consumed by the package export/import wizard (`package-wizard.ts`,
 * `package-export.ts`), so both are reachable and listed here; `PACKAGE_ROUND_TRIP_ERROR_CODES`
 * names them so the parity gate can assert they stay covered.
 *
 * `server_rejected` (frozen 2026-08-31, `error-mapping-v1.md`) is a permanent failure
 * (`recoverable=false`, "不得提示重试"); every surface that can receive it (operations, conversion
 * jobs, the collab `rejected` frame) classifies it through this registry, so the parity gate
 * covers its zh/en keys like every other stable code. */
export const FLOW_ERROR_CODES = [
	'unauthenticated',
	'forbidden',
	'feature_disabled',
	'not_found',
	'unsupported_protocol',
	'stale_frontier',
	'invalid_update',
	'policy_rejected',
	'limit_exceeded',
	'resync_required',
	'authorization_churn',
	'server_draining',
	'server_rejected',
	'checksum_mismatch',
	'unsupported_format'
] as const satisfies readonly FlowErrorCode[];

/** The stable codes whose only producer is the v0.8 import/export REST surface
 * (`error-mapping-v1.md`: "n/a；import/export REST only"). Both are permanent failures
 * (`recoverable=false`): the package has to be obtained again or produced by a compatible
 * exporter. */
export const PACKAGE_ROUND_TRIP_ERROR_CODES = ['checksum_mismatch', 'unsupported_format'] as const;

/** `server_draining`'s frozen, REQUIRED discriminator values. */
export const SERVER_DRAINING_REASONS = ['drain', 'contention'] as const;
export type ServerDrainingReason = (typeof SERVER_DRAINING_REASONS)[number];

/** The WebSocket close code the server uses to end a session because the workspace is draining
 * (`apps/api/src/flow/collab/registry.rs::DRAIN_CLOSE_CODE`, asserted there as 4410;
 * `error-mapping-v1.md`: "`reason=drain`：4410 关闭并按 retry 重连"). `contention` has no close
 * code at all -- that is the whole point of the distinction. */
export const DRAIN_CLOSE_CODE = 4410;

/** `error-mapping-v1.md`'s `server_rejected` row: "保持连接；**连续 3 次**关闭 4500". The server
 * closes with it (`apps/api/src/flow/collab/session.rs::SERVER_REJECTED_CLOSE_CODE`) and this client
 * closes with it too when its own count of consecutive refusals reaches the same threshold. */
export const SERVER_REJECTED_CLOSE_CODE = 4500;

/** The frozen "连续 3 次" threshold shared by the client and the server. */
export const SERVER_REJECTED_STREAK_LIMIT = 3;

/** `invalid_update.details.reason` values this client branches on (`error-mapping-v1.md`'s
 * approved table). Any other value -- or none -- is the plain `invalid_update`. */
export const INVALID_UPDATE_REASONS = [
	'subtree_spans_multiple_projects',
	'child_project_must_match_parent',
	'project_unmapped',
	'idempotency_key_body_drift',
	'invalid_field_value'
] as const;
export type InvalidUpdateReason = (typeof INVALID_UPDATE_REASONS)[number];

/** What the caller must do for one named `invalid_update` reason (the table's "调用方该做什么"
 * column). `integrity_repair` is the defensive reason: stop ordinary writes and hand over to the
 * workspace administrator's integrity tools. */
export type InvalidUpdateAction =
	| 'integrity_repair'
	| 'change_project'
	| 'remap_project'
	| 'rekey_and_resend'
	| 'fix_field_value';

export interface InvalidUpdateDisposition {
	readonly reason: InvalidUpdateReason;
	readonly action: InvalidUpdateAction;
	readonly i18nKey: string;
}

const INVALID_UPDATE_ACTIONS: Readonly<Record<InvalidUpdateReason, InvalidUpdateAction>> = {
	subtree_spans_multiple_projects: 'integrity_repair',
	child_project_must_match_parent: 'change_project',
	project_unmapped: 'remap_project',
	idempotency_key_body_drift: 'rekey_and_resend',
	invalid_field_value: 'fix_field_value'
};

/**
 * The disposition for an `invalid_update` that carries one of the approved `details.reason`
 * values, or `null` for any other error, an absent reason, or a reason this client does not know.
 *
 * `error-mapping-v1.md`: "consumer 不得因为不认识某个 `reason` 就改变错误分类——未知 reason 一律按
 * 无 reason 的 `invalid_update` 处理（`recoverable=false`），**不得用 message 补判**". So `null` means
 * "the unified fallback", never a guess.
 */
export function invalidUpdateDisposition(
	error: Pick<FlowError, 'code' | 'details'>
): InvalidUpdateDisposition | null {
	if (error.code !== 'invalid_update') return null;
	if (typeof error.details !== 'object' || error.details === null) return null;
	const reason = (error.details as { reason?: unknown }).reason;
	if (typeof reason !== 'string') return null;
	if (!(INVALID_UPDATE_REASONS as readonly string[]).includes(reason)) return null;
	const known = reason as InvalidUpdateReason;
	return {
		reason: known,
		action: INVALID_UPDATE_ACTIONS[known],
		i18nKey: `flow.error.invalid_update_reason.${known}`
	};
}

function isFlowErrorCode(value: unknown): value is FlowErrorCode {
	return typeof value === 'string' && (FLOW_ERROR_CODES as readonly string[]).includes(value);
}

/**
 * Reads `server_draining`'s required `details.reason`. Returns `null` for a missing, non-string
 * or unrecognised value -- never a default.
 *
 * Defaulting is the specific failure `error-mapping-v1.md` forbids ("两者不得互换，缺失/未知
 * reason 违反协议"): a `contention` default would let a malformed drain notice render as an
 * ordinary "retrying" state and leave the client hammering an instance that is shutting down,
 * and a `drain` default would show a maintenance banner for a routine lock conflict.
 */
export function parseServerDrainingReason(details: unknown): ServerDrainingReason | null {
	if (typeof details !== 'object' || details === null) return null;
	const reason = (details as { reason?: unknown }).reason;
	if (typeof reason !== 'string') return null;
	return (SERVER_DRAINING_REASONS as readonly string[]).includes(reason)
		? (reason as ServerDrainingReason)
		: null;
}

/** `details.retry_after_ms` when the producer supplied a usable one, else `null`. It is a LOWER
 * bound on the client's own backoff (`ui-surface-v1.md` step 6: "`server_draining.details.
 * retry_after_ms` 是下限"), never an upper bound and never a substitute for backoff. */
export function parseRetryAfterMs(details: unknown): number | null {
	if (typeof details !== 'object' || details === null) return null;
	const value = (details as { retry_after_ms?: unknown }).retry_after_ms;
	if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) return null;
	return value;
}

/**
 * How a `server_draining` rejection must change this client's behaviour. The two reasons share a
 * stable code and differ in exactly the four ways `gates/gate-commands.md`'s Error verifier
 * paragraph enumerates for the UI: i18n key, retry state, whether the socket survives, and
 * whether the accepted head is kept.
 */
export interface DrainDisposition {
	readonly reason: ServerDrainingReason;
	/** `flow.error.server_draining.<reason>`; the two are never the same string. */
	readonly i18nKey: string;
	/** Sync indicator state to show. `drain` is a maintenance/reconnect state; `contention` must
	 * NOT impersonate one (`error-mapping-v1.md`: "显示重试中且不冒充维护"), so it stays on the
	 * local-unsaved state with the live connection untouched. */
	readonly syncState: Extract<SyncState, 'reconnecting' | 'local'>;
	/** `drain` closes (4410) and the client waits for a NEW connection; `contention` keeps the
	 * existing one (`ui-surface-v1.md` step 6: "保持现有连接/accepted head"). */
	readonly keepConnection: boolean;
	/** Whether the client should re-run its own connect loop. Only `drain` does. */
	readonly reconnects: boolean;
	/** The close code that accompanies this reason, or `null` when the connection is kept. */
	readonly closeCode: number | null;
	/** Lower bound on the next reconnect delay, when the producer supplied one. */
	readonly retryAfterMs: number | null;
}

const DRAIN_DISPOSITIONS: Readonly<
	Record<ServerDrainingReason, Omit<DrainDisposition, 'retryAfterMs'>>
> = {
	drain: {
		reason: 'drain',
		i18nKey: 'flow.error.server_draining.drain',
		syncState: 'reconnecting',
		keepConnection: false,
		reconnects: true,
		closeCode: DRAIN_CLOSE_CODE
	},
	contention: {
		reason: 'contention',
		i18nKey: 'flow.error.server_draining.contention',
		syncState: 'local',
		keepConnection: true,
		reconnects: false,
		closeCode: null
	}
};

/** The disposition for a `server_draining` error, or `null` when the required discriminator is
 * missing/unknown -- i.e. the producer violated the protocol and the caller must fail closed
 * rather than guess. */
export function drainDisposition(
	error: Pick<FlowError, 'code' | 'details'>
): DrainDisposition | null {
	if (error.code !== 'server_draining') return null;
	const reason = parseServerDrainingReason(error.details);
	if (!reason) return null;
	return { ...DRAIN_DISPOSITIONS[reason], retryAfterMs: parseRetryAfterMs(error.details) };
}

/**
 * The i18n key for any Flow error.
 *
 * `server_draining` is the one discriminated code: a valid `details.reason` selects one of the
 * two reason keys, and `ui-surface-v1.md` forbids rendering the base `flow.error.server_draining`
 * for a legal response -- which is why no such key exists in `zh.json`/`en.json` at all.
 *
 * A `server_draining` whose required discriminator is missing or unknown is not a legal response.
 * It is failed closed onto `flow.error.unsupported_protocol` ("要求刷新客户端"): this peer sent a
 * frame shape the frozen protocol does not define, and the honest thing to tell the user is that
 * the client cannot interpret it -- NOT to silently render it as `contention`, which is what a
 * `reason === 'drain' ? drain : contention` ternary does and which would understate a real drain.
 */
export function flowErrorI18nKey(error: Pick<FlowError, 'code' | 'details'>): string {
	if (error.code === 'server_draining') {
		return drainDisposition(error)?.i18nKey ?? 'flow.error.unsupported_protocol';
	}
	return `flow.error.${error.code}`;
}

/**
 * `flowErrorI18nKey`, refined by a named `invalid_update.details.reason` where the surface acts on
 * it (the object page banner, the navigator's create, the recovery draft). An unknown or absent
 * reason falls back to the stable code's key -- the unified fallback, never a message-based guess.
 */
export function flowErrorDetailKey(error: Pick<FlowError, 'code' | 'details'>): string {
	return invalidUpdateDisposition(error)?.i18nKey ?? flowErrorI18nKey(error);
}

/** The REST envelope's error-carrying fields (`apps/api/src/error.rs`: `ApiResponse` gains
 * `error_code` + `details` for `ApiError::Typed`; transport status stays HTTP 200 and the
 * envelope `code` is the business code). */
export interface FlowErrorEnvelope {
	readonly code: number;
	readonly error_code?: string | null;
	readonly details?: unknown;
}

/**
 * Maps a REST envelope onto a `FlowError`, or `null` when it is not a Flow business rejection.
 *
 * Only the machine-readable `error_code` is consulted. An envelope carrying a non-zero business
 * code but no recognised `error_code` yields `null` rather than a guess -- callers keep their own
 * generic handling for those instead of this module inventing a Flow semantic for them.
 */
export function flowErrorFromEnvelope(envelope: FlowErrorEnvelope): FlowError | null {
	if (!isFlowErrorCode(envelope.error_code)) return null;
	const code = envelope.error_code;
	return { code, recoverable: isRecoverable(code), details: envelope.details, origin: 'server' };
}

/** Maps a WebSocket `rejected` control frame onto a `FlowError`, or `null` when the frame does
 * not carry a stable code this client knows. */
export function flowErrorFromRejectedFrame(frame: Record<string, unknown>): FlowError | null {
	if (!isFlowErrorCode(frame.code)) return null;
	const code = frame.code;
	return {
		code,
		recoverable: typeof frame.recoverable === 'boolean' ? frame.recoverable : isRecoverable(code),
		details: frame.details,
		origin: 'server'
	};
}

/**
 * Maps a WebSocket close onto a `FlowError`, or `null` when the close code is not one of the
 * frozen Flow close codes.
 *
 * The drain close (4410) carries its `{reason,retry_after_ms}` JSON in the close `reason` string
 * (`apps/api/src/flow/collab/frame.rs::DrainSignal::close_reason`). A 4410 whose payload does not
 * parse still yields `server_draining` with whatever details were readable -- the close code
 * itself is a frozen, machine-readable discriminator, so it does not need the body to be valid --
 * but the resulting error then fails `drainDisposition` and is handled as a protocol violation,
 * not silently treated as a drain.
 */
export function flowErrorFromCloseCode(closeCode: number, closeReason: string): FlowError | null {
	switch (closeCode) {
		case 4401:
			return { code: 'unauthenticated', recoverable: true, origin: 'server' };
		case 4403:
			return { code: 'forbidden', recoverable: false, origin: 'server' };
		case 4404:
			return { code: 'feature_disabled', recoverable: false, origin: 'server' };
		case 4406:
			return { code: 'unsupported_protocol', recoverable: false, origin: 'server' };
		case SERVER_REJECTED_CLOSE_CODE:
			// Three consecutive permanent refusals: the session is given up for good, so this is
			// `server_rejected` (recoverable=false), never a reconnect trigger.
			return { code: 'server_rejected', recoverable: false, origin: 'server' };
		case DRAIN_CLOSE_CODE:
			return {
				code: 'server_draining',
				recoverable: true,
				details: parseCloseReason(closeReason),
				origin: 'server'
			};
		default:
			return null;
	}
}

/**
 * A locally-synthesised error: this build's own condition, never something a peer reported.
 *
 * Always used for conditions with no wire counterpart (a connect that never completed, a session
 * torn down with writes still in flight). Marking them keeps `server_draining`'s
 * server-produced-discriminator guarantee assertable -- see `FlowError.origin`.
 */
export function clientError(code: FlowErrorCode, details?: unknown): FlowError {
	return { code, recoverable: isRecoverable(code), details, origin: 'client' };
}

/** Whether this error was read off a wire surface rather than synthesised locally. */
export function isServerReported(error: Pick<FlowError, 'origin'>): boolean {
	return error.origin === 'server';
}

function parseCloseReason(closeReason: string): unknown {
	if (closeReason === '') return undefined;
	try {
		return JSON.parse(closeReason) as unknown;
	} catch {
		return undefined;
	}
}

/** `error-mapping-v1.md`'s per-code `recoverable`, for the surfaces that do not transmit it. */
function isRecoverable(code: FlowErrorCode): boolean {
	switch (code) {
		case 'unauthenticated':
		case 'stale_frontier':
		case 'resync_required':
		case 'authorization_churn':
		case 'server_draining':
			return true;
		default:
			return false;
	}
}
