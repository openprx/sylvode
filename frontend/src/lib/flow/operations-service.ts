// State machines for the workspace Flow operations panel
// (`/workspace/{id}/settings/flow/operations`; `contracts/rest-api-v1.md` v0.8 table,
// `surface-coverage-v1.md` admin health / lag / integrity dashboards, verify operation,
// compact preview/confirm, rebuild preview/confirm, admin operations panel, projection-lag badge).
//
// Reads go through `FlowObjectRepository`, writes through `FlowCommandService`; components only
// hold ephemeral UI state. Rules this module owns, so they are unit-testable without a component:
//
// - Dangerous operations (compact, rebuild-projection, delivery replay) are two-step: a dry-run
//   receipt first, then an execute that is only possible while that receipt is current, the
//   expected head is set, and (compact/rebuild) the user typed the exact target id. `execute`
//   refuses -- and sends nothing -- when any of that is missing.
// - Verify is dry-run only: every request carries `dry_run: true`.
// - Idempotency: every intent mints its key once and a retry of that intent resends it. The
//   dry-run and the execute are SEPARATE intents with separate keys. That is a backend fact, not a
//   style choice: `apps/api/src/flow/operations.rs::request_hash` and
//   `routes/flow.rs::post_flow_delivery_replay` both hash `dry_run` into the request body, so
//   reusing the dry-run key for the execute is answered with 409 "idempotency key body drift"
//   (reproduced against the real API; see `task/openpr/receipt-fp-N3.md`).
// - Errors branch on `error_code`, or on the numeric code for the legacy envelope that carries no
//   `error_code` -- never on `message`. `server_rejected` (and any typed code this build does not
//   know) is a permanent failure: no retry is offered and the operation is closed.
// - `delivery_cancelled` is a normal cancellation and is never counted as dead-letter.

import type { ApiResult } from '$lib/api/client';
import type {
	FlowAdminHealth,
	FlowAdminIntegrity,
	FlowAdminLag,
	FlowOperationReceipt,
	FlowProjectionLag,
	FlowReplayMode,
	FlowReplayResponse,
	FlowReplayWindow,
	ReplayDeliveriesInput
} from '$lib/api/flow';
import { FlowCommandService } from './command-service';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';
import { resolveWorkspaceAdmin } from './flow-settings';
import { FlowObjectRepository } from './object-repository';
import { newIdempotencyKey } from './package-export';

/** Health auto-refresh period (task FP-N3: "每 15s 自动刷新"). */
export const OPS_HEALTH_REFRESH_MS = 15_000;
/** `contracts/limits-v1.md` `replay_max_window_days` (frozen 30; the server enforces the same
 * value in `events/dispatcher.rs::REPLAY_MAX_WINDOW_DAYS`). The local check only saves a round
 * trip; the server still refuses anything outside the window. */
export const REPLAY_MAX_WINDOW_DAYS = 30;
/** `GET .../integrity` accepts `limit` 1..=100 and no cursor at this baseline. */
export const OPS_INTEGRITY_LIMITS = [50, 100] as const;
export type OpsIntegrityLimit = (typeof OPS_INTEGRITY_LIMITS)[number];
export const OPS_LAG_PAGE_SIZE = 20;

const DAY_MS = 86_400_000;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function isUuid(value: string): boolean {
	return UUID_RE.test(value.trim());
}

// ---- error classification ---------------------------------------------------------------------

export interface OpsFailure {
	readonly kind: 'forbidden' | 'failed';
	readonly messageKey: string;
	/** The same intent may be resent (same key). */
	readonly retryable: boolean;
	/** No retry and no further attempt in this operation (`server_rejected`, unknown typed codes,
	 * typed non-recoverable codes, forbidden). */
	readonly permanent: boolean;
}

/**
 * Classifies a non-success envelope. `error_code` first; the legacy envelope
 * (`apps/api/src/error.rs::legacy_response`, which every operations handler uses for its
 * BadRequest/Forbidden/NotFound/Conflict) is classified by numeric code. `message` is never read
 * -- the server puts `stale_frontier` and `idempotency key body drift` there, and both are a 409
 * here, which is why the conflict copy tells the user to run a fresh dry-run.
 */
export function classifyOperationsError(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): OpsFailure {
	const flowError = flowErrorFromEnvelope(result);
	if (flowError) {
		if (flowError.code === 'forbidden') {
			return {
				kind: 'forbidden',
				messageKey: 'flow.error.forbidden',
				retryable: false,
				permanent: true
			};
		}
		return {
			kind: 'failed',
			messageKey: flowErrorI18nKey(flowError),
			retryable: flowError.recoverable,
			permanent: !flowError.recoverable
		};
	}
	if (typeof result.error_code === 'string') {
		return failed('flow.operations.error.rejected', false, true);
	}
	switch (result.code) {
		case 401:
			return failed('flow.error.unauthenticated', false, true);
		case 403:
			return {
				kind: 'forbidden',
				messageKey: 'flow.error.forbidden',
				retryable: false,
				permanent: true
			};
		case 404:
			return failed('flow.operations.error.notFound', false, false);
		case 409:
			return failed('flow.operations.error.conflict', false, false);
		default:
			return result.code >= 500
				? failed('flow.operations.error.unavailable', true, false)
				: failed('flow.operations.error.rejected', false, false);
	}
}

function failed(messageKey: string, retryable: boolean, permanent: boolean): OpsFailure {
	return { kind: 'failed', messageKey, retryable, permanent };
}

/** Page-level reads: a 403 or 404 (non-member) lands on the forbidden-safe state, so the page
 * does not reveal whether the workspace exists. */
function isForbiddenSafe(result: Pick<ApiResult<unknown>, 'code' | 'error_code'>): boolean {
	if (result.error_code === 'forbidden' || result.error_code === 'not_found') return true;
	return (
		(result.error_code === undefined || result.error_code === null) &&
		(result.code === 403 || result.code === 404)
	);
}

export type OpsRead<T> =
	| { readonly status: 'ready'; readonly data: T }
	| { readonly status: 'forbidden' }
	| { readonly status: 'failed'; readonly failure: OpsFailure };

function toRead<T>(result: ApiResult<T>): OpsRead<T> {
	if (result.code === 0 && result.data) return { status: 'ready', data: result.data };
	if (isForbiddenSafe(result)) return { status: 'forbidden' };
	return { status: 'failed', failure: classifyOperationsError(result) };
}

// ---- health -----------------------------------------------------------------------------------

/** Dead-letter = `dispatch_failed + delivery_failed`. `delivery_cancelled` is deliberately NOT
 * part of it (`rest-api-v1.md`: "单独计、不进 dead-letter、不触发告警"). */
export function deadLetterTotal(health: Pick<FlowAdminHealth, 'dead_letter'>): number {
	return health.dead_letter.dispatch_failed + health.dead_letter.delivery_failed;
}

/** The dead-letter card uses alert styling only when something is actually dead-lettered. */
export function deadLetterAlert(health: Pick<FlowAdminHealth, 'dead_letter'>): boolean {
	return deadLetterTotal(health) > 0;
}

export const HEALTH_STATUS_KEYS: Readonly<Record<string, string>> = {
	healthy: 'flow.operations.health.status.healthy',
	degraded: 'flow.operations.health.status.degraded'
};

export const INTEGRITY_STATUS_KEYS: Readonly<Record<string, string>> = {
	healthy: 'flow.operations.integrity.status.healthy',
	integrity_error: 'flow.operations.integrity.status.integrity_error'
};

export function statusKey(map: Readonly<Record<string, string>>, status: string): string {
	return map[status] ?? 'flow.operations.status.unknown';
}

export interface HealthSnapshot {
	readonly health: FlowAdminHealth | null;
	/** True when the last refresh failed and `health` is the previous successful sample. */
	readonly stale: boolean;
	readonly failure: OpsFailure | null;
	readonly forbidden: boolean;
	readonly paused: boolean;
	readonly loading: boolean;
	readonly updatedAt: number | null;
}

export type Scheduler = (run: () => void, ms: number) => () => void;

const defaultScheduler: Scheduler = (run, ms) => {
	const handle = setInterval(run, ms);
	return () => clearInterval(handle);
};

/** Polls workspace health every `OPS_HEALTH_REFRESH_MS`; can be paused. A failed refresh keeps
 * the previous sample and marks it stale instead of blanking the dashboard. */
export class HealthMonitor {
	private health: FlowAdminHealth | null = null;
	private failure: OpsFailure | null = null;
	private forbidden = false;
	private paused = false;
	private loading = false;
	private updatedAt: number | null = null;
	private cancelTimer: (() => void) | null = null;
	private generation = 0;

	constructor(
		private readonly workspaceId: string,
		private readonly reads: Pick<FlowObjectRepository, 'getAdminHealth'>,
		private readonly onChange: (snapshot: HealthSnapshot) => void,
		private readonly schedule: Scheduler = defaultScheduler,
		private readonly now: () => number = Date.now
	) {}

	snapshot(): HealthSnapshot {
		return {
			health: this.health,
			stale: this.health !== null && this.failure !== null,
			failure: this.failure,
			forbidden: this.forbidden,
			paused: this.paused,
			loading: this.loading,
			updatedAt: this.updatedAt
		};
	}

	async refresh(): Promise<HealthSnapshot> {
		const generation = ++this.generation;
		this.loading = true;
		this.emit();
		let read: OpsRead<FlowAdminHealth>;
		try {
			read = toRead(await this.reads.getAdminHealth(this.workspaceId));
		} catch {
			read = {
				status: 'failed',
				failure: failed('flow.operations.error.unavailable', true, false)
			};
		}
		if (generation !== this.generation) return this.snapshot();
		this.loading = false;
		if (read.status === 'ready') {
			this.health = read.data;
			this.failure = null;
			this.updatedAt = this.now();
		} else if (read.status === 'forbidden') {
			this.forbidden = true;
			this.stopTimer();
		} else {
			this.failure = read.failure;
		}
		this.emit();
		return this.snapshot();
	}

	start(): void {
		if (this.forbidden) return;
		this.paused = false;
		this.stopTimer();
		this.cancelTimer = this.schedule(() => void this.refresh(), OPS_HEALTH_REFRESH_MS);
		this.emit();
	}

	pause(): void {
		this.paused = true;
		this.stopTimer();
		this.emit();
	}

	resume(): void {
		this.start();
		void this.refresh();
	}

	dispose(): void {
		this.stopTimer();
		this.generation += 1;
	}

	private stopTimer(): void {
		this.cancelTimer?.();
		this.cancelTimer = null;
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}

// ---- maintenance operations (verify / compact / rebuild-projection) ---------------------------

export type MaintenanceKind = 'verify' | 'compact' | 'rebuild';

export const MAINTENANCE_TITLE_KEYS: Readonly<Record<MaintenanceKind, string>> = {
	verify: 'flow.operations.drawer.title.verify',
	compact: 'flow.operations.drawer.title.compact',
	rebuild: 'flow.operations.drawer.title.rebuild'
};

export const OPERATION_NAME_KEYS: Readonly<Record<string, string>> = {
	verify_document: 'flow.operations.drawer.title.verify',
	compact: 'flow.operations.drawer.title.compact',
	rebuild_projection: 'flow.operations.drawer.title.rebuild'
};

/** Labels for the `result` fields each operation returns (`apps/api/src/flow/operations.rs`,
 * `maintenance.rs::ProjectionRebuildResult`, `collab/integrity.rs::DocumentFingerprint`). A field
 * the server adds later is still shown, under its raw name. */
export const RECEIPT_FIELD_KEYS: Readonly<Record<string, string>> = {
	deep: 'flow.operations.receipt.field.deep',
	head_seq: 'flow.operations.receipt.field.head_seq',
	head_frontier: 'flow.operations.receipt.field.head_frontier',
	snapshot_seq: 'flow.operations.receipt.field.snapshot_seq',
	tail_updates: 'flow.operations.receipt.field.tail_updates',
	tail_bytes: 'flow.operations.receipt.field.tail_bytes',
	would_compact: 'flow.operations.receipt.field.would_compact',
	semantic_hash: 'flow.operations.receipt.field.semantic_hash',
	snapshot_checksum: 'flow.operations.receipt.field.snapshot_checksum',
	deleted_updates: 'flow.operations.receipt.field.deleted_updates',
	lagging_clients: 'flow.operations.receipt.field.lagging_clients',
	forced_resync_clients: 'flow.operations.receipt.field.forced_resync_clients',
	before_hash: 'flow.operations.receipt.field.before_hash',
	after_hash: 'flow.operations.receipt.field.after_hash',
	changed: 'flow.operations.receipt.field.changed',
	executed: 'flow.operations.receipt.field.executed',
	projection_seq: 'flow.operations.receipt.field.projection_seq',
	workspace_id: 'flow.operations.receipt.field.workspace_id',
	object_id: 'flow.operations.receipt.field.object_id',
	document_id: 'flow.operations.receipt.field.document_id'
};

export interface ReceiptRow {
	readonly field: string;
	/** `null` for a field this build has no label for; the UI shows `field` verbatim. */
	readonly labelKey: string | null;
	readonly value: string;
}

/** Flattens `receipt.result` (one nested level, e.g. verify's `fingerprint`) into display rows. */
export function receiptRows(receipt: Pick<FlowOperationReceipt, 'result'>): ReceiptRow[] {
	const rows: ReceiptRow[] = [];
	const push = (field: string, value: unknown) => {
		const leaf = field.split('.').pop() ?? field;
		rows.push({
			field,
			labelKey: RECEIPT_FIELD_KEYS[leaf] ?? null,
			value: value === null || value === undefined ? '-' : String(value)
		});
	};
	for (const [key, value] of Object.entries(receipt.result ?? {})) {
		if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
			for (const [inner, innerValue] of Object.entries(value as Record<string, unknown>)) {
				push(`${key}.${inner}`, innerValue);
			}
		} else {
			push(key, value);
		}
	}
	return rows;
}

/** A non-negative safe integer, or `null` for blank/invalid text. */
export function parseHeadSeq(text: string): number | null {
	const trimmed = text.trim();
	if (!/^\d+$/.test(trimmed)) return null;
	const value = Number(trimmed);
	return Number.isSafeInteger(value) ? value : null;
}

export interface MaintenanceDeps {
	readonly commands: Pick<
		FlowCommandService,
		'verifyDocument' | 'compactDocument' | 'rebuildProjection'
	>;
	readonly newKey: () => string;
}

export interface MaintenanceSnapshot {
	readonly kind: MaintenanceKind;
	readonly targetId: string;
	readonly deep: boolean;
	readonly expectedHeadText: string;
	readonly expectedHead: number | null;
	readonly confirmText: string;
	readonly confirmMatches: boolean;
	/** The current dry-run receipt (always `dry_run: true`). */
	readonly preview: FlowOperationReceipt | null;
	/** The execute receipt (always `dry_run: false`). Compact/rebuild only. */
	readonly executed: FlowOperationReceipt | null;
	readonly busy: boolean;
	readonly failure: OpsFailure | null;
	/** Which step a retry button would resend (same key), or `null` when no retry is offered. */
	readonly retry: 'dry_run' | 'execute' | null;
	readonly closed: boolean;
	readonly canDryRun: boolean;
	readonly canExecute: boolean;
}

export type MaintenanceOutcome =
	| { readonly status: 'ok'; readonly receipt: FlowOperationReceipt }
	| { readonly status: 'refused' }
	| { readonly status: 'failed'; readonly failure: OpsFailure };

interface Intent {
	readonly key: string;
	readonly fingerprint: string;
}

/**
 * One verify/compact/rebuild operation against one target (document id for verify/compact,
 * object id for rebuild), from first dry-run to execute.
 */
export class MaintenanceOperation {
	private deep = false;
	private headText: string;
	private confirmText = '';
	private preview: FlowOperationReceipt | null = null;
	private executed: FlowOperationReceipt | null = null;
	private dryRunIntent: Intent | null = null;
	private executeIntent: Intent | null = null;
	private failure: OpsFailure | null = null;
	private retry: 'dry_run' | 'execute' | null = null;
	private busy = false;
	private closed = false;

	constructor(
		readonly kind: MaintenanceKind,
		readonly targetId: string,
		private readonly deps: MaintenanceDeps,
		initialExpectedHead: number | null = null
	) {
		this.headText = initialExpectedHead === null ? '' : String(initialExpectedHead);
	}

	setDeep(deep: boolean): void {
		if (this.kind !== 'verify' || deep === this.deep) return;
		this.deep = deep;
		this.invalidate();
	}

	/** Any change to the expected head voids the current dry-run receipt. */
	setExpectedHead(text: string): void {
		if (text === this.headText) return;
		this.headText = text;
		this.invalidate();
	}

	setConfirmText(text: string): void {
		this.confirmText = text;
	}

	get expectedHead(): number | null {
		return parseHeadSeq(this.headText);
	}

	canDryRun(): boolean {
		if (this.busy || this.closed || this.executed !== null) return false;
		if (this.kind === 'verify') return this.headText.trim() === '' || this.expectedHead !== null;
		return this.expectedHead !== null;
	}

	/**
	 * Execute needs ALL of: a current dry-run receipt, an expected head equal to the one that
	 * receipt was taken at, and the user-typed confirmation equal to the target id.
	 */
	canExecute(): boolean {
		if (this.kind === 'verify') return false;
		if (this.busy || this.closed || this.executed !== null) return false;
		const preview = this.preview;
		if (preview === null || preview.dry_run !== true) return false;
		const head = this.expectedHead;
		if (head === null || preview.expected_head_seq !== head) return false;
		return this.confirmMatches();
	}

	confirmMatches(): boolean {
		return this.confirmText.trim() === this.targetId;
	}

	snapshot(): MaintenanceSnapshot {
		return {
			kind: this.kind,
			targetId: this.targetId,
			deep: this.deep,
			expectedHeadText: this.headText,
			expectedHead: this.expectedHead,
			confirmText: this.confirmText,
			confirmMatches: this.confirmMatches(),
			preview: this.preview,
			executed: this.executed,
			busy: this.busy,
			failure: this.failure,
			retry: this.retry,
			closed: this.closed,
			canDryRun: this.canDryRun(),
			canExecute: this.canExecute()
		};
	}

	async dryRun(): Promise<MaintenanceOutcome> {
		if (!this.canDryRun()) return { status: 'refused' };
		const head = this.expectedHead;
		const fingerprint = JSON.stringify({ deep: this.deep, head });
		if (!this.dryRunIntent || this.dryRunIntent.fingerprint !== fingerprint) {
			this.dryRunIntent = { key: this.deps.newKey(), fingerprint };
		}
		const intent = this.dryRunIntent;
		this.busy = true;
		this.failure = null;
		this.retry = null;
		let result: ApiResult<FlowOperationReceipt>;
		try {
			result = await this.sendDryRun(head, intent.key);
		} catch {
			result = { code: 503, message: '', data: null };
		}
		this.busy = false;
		if (result.code === 0 && result.data) {
			if (result.data.dry_run !== true) {
				// A dry-run request answered with a non-dry-run receipt is never shown as a preview.
				return this.fail(failed('flow.operations.error.unexpectedResponse', false, true), null);
			}
			this.dryRunIntent = null;
			this.preview = result.data;
			return { status: 'ok', receipt: result.data };
		}
		return this.fail(classifyOperationsError(result), 'dry_run');
	}

	async execute(): Promise<MaintenanceOutcome> {
		if (!this.canExecute()) return { status: 'refused' };
		const preview = this.preview as FlowOperationReceipt;
		const fingerprint = preview.operation_id;
		if (!this.executeIntent || this.executeIntent.fingerprint !== fingerprint) {
			this.executeIntent = { key: this.deps.newKey(), fingerprint };
		}
		const intent = this.executeIntent;
		const confirmation = this.confirmText.trim();
		this.busy = true;
		this.failure = null;
		this.retry = null;
		let result: ApiResult<FlowOperationReceipt>;
		try {
			result =
				this.kind === 'compact'
					? await this.deps.commands.compactDocument(this.targetId, {
							dry_run: false,
							expected_head_seq: preview.expected_head_seq,
							confirm_document_id: confirmation,
							idempotency_key: intent.key
						})
					: await this.deps.commands.rebuildProjection(this.targetId, {
							dry_run: false,
							expected_head_seq: preview.expected_head_seq,
							confirm_object_id: confirmation,
							idempotency_key: intent.key
						});
		} catch {
			result = { code: 503, message: '', data: null };
		}
		this.busy = false;
		if (result.code === 0 && result.data) {
			if (result.data.dry_run !== false) {
				return this.fail(failed('flow.operations.error.unexpectedResponse', false, true), null);
			}
			this.executeIntent = null;
			this.executed = result.data;
			return { status: 'ok', receipt: result.data };
		}
		const failure = classifyOperationsError(result);
		if (!failure.retryable) {
			// The receipt no longer describes what the server would do (stale head, changed state):
			// a new dry-run is required before another execute.
			this.preview = null;
			this.executeIntent = null;
		}
		return this.fail(failure, 'execute');
	}

	private sendDryRun(head: number | null, key: string): Promise<ApiResult<FlowOperationReceipt>> {
		switch (this.kind) {
			case 'verify':
				return this.deps.commands.verifyDocument(this.targetId, {
					dry_run: true,
					deep: this.deep,
					...(head === null ? {} : { expected_head_seq: head }),
					idempotency_key: key
				});
			case 'compact':
				return this.deps.commands.compactDocument(this.targetId, {
					dry_run: true,
					expected_head_seq: head as number,
					idempotency_key: key
				});
			case 'rebuild':
				return this.deps.commands.rebuildProjection(this.targetId, {
					dry_run: true,
					expected_head_seq: head as number,
					idempotency_key: key
				});
		}
	}

	private fail(failure: OpsFailure, step: 'dry_run' | 'execute' | null): MaintenanceOutcome {
		this.failure = failure;
		if (failure.permanent) {
			this.closed = true;
			this.dryRunIntent = null;
			this.executeIntent = null;
		}
		this.retry = failure.retryable && !failure.permanent ? step : null;
		if (step === 'dry_run' && !failure.retryable) this.dryRunIntent = null;
		return { status: 'failed', failure };
	}

	private invalidate(): void {
		this.preview = null;
		this.dryRunIntent = null;
		this.executeIntent = null;
		this.failure = null;
		this.retry = null;
	}
}

// ---- delivery replay --------------------------------------------------------------------------

export const REPLAY_MODES: readonly FlowReplayMode[] = ['rebuild', 'requeue_failed'];

export const REPLAY_MODE_KEYS: Readonly<Record<FlowReplayMode, string>> = {
	rebuild: 'flow.operations.replay.mode.rebuild',
	requeue_failed: 'flow.operations.replay.mode.requeue_failed'
};

export interface ReplayForm {
	readonly mode: FlowReplayMode;
	readonly eventType: string;
	/** `''` = any subscriber kind; the server only supports `webhook`. */
	readonly subscriberKind: '' | 'webhook';
	readonly subscriberId: string;
	/** Anything `Date` parses; the page uses `datetime-local` values (browser local time). */
	readonly from: string;
	readonly to: string;
}

export type ReplayRequest = Omit<ReplayDeliveriesInput, 'dry_run' | 'idempotency_key'>;

export type ReplayValidation =
	| { readonly ok: true; readonly request: ReplayRequest }
	| { readonly ok: false; readonly errorKey: string };

/**
 * Mirrors `events/dispatcher.rs::validate_replay_window`: `from < to`, `to <= now`, and
 * `from > now - replay_max_window_days`. Anything outside is refused locally with no request.
 */
export function validateReplayForm(form: ReplayForm, now: number): ReplayValidation {
	const from = new Date(form.from).getTime();
	const to = new Date(form.to).getTime();
	if (form.from.trim() === '' || form.to.trim() === '' || Number.isNaN(from) || Number.isNaN(to)) {
		return { ok: false, errorKey: 'flow.operations.replay.error.invalidDate' };
	}
	if (from >= to) return { ok: false, errorKey: 'flow.operations.replay.error.empty' };
	if (to > now) return { ok: false, errorKey: 'flow.operations.replay.error.future' };
	if (from <= now - REPLAY_MAX_WINDOW_DAYS * DAY_MS) {
		return { ok: false, errorKey: 'flow.operations.replay.error.tooOld' };
	}
	const subscriberId = form.subscriberId.trim();
	if (subscriberId !== '' && !isUuid(subscriberId)) {
		return { ok: false, errorKey: 'flow.operations.replay.error.invalidSubscriber' };
	}
	const eventType = form.eventType.trim();
	return {
		ok: true,
		request: {
			mode: form.mode,
			...(eventType === '' ? {} : { event_type: eventType }),
			...(form.subscriberKind === '' ? {} : { subscriber_kind: form.subscriberKind }),
			...(subscriberId === '' ? {} : { subscriber_id: subscriberId }),
			from: new Date(from).toISOString(),
			to: new Date(to).toISOString(),
			confirm: true
		}
	};
}

export type ReplayOutcomeView =
	| {
			readonly mode: 'rebuild';
			readonly replayed: number;
			readonly skippedAlreadyDelivered: number;
			readonly deliveryIds: readonly string[];
			readonly window: FlowReplayWindow;
	  }
	| {
			readonly mode: 'requeue_failed';
			readonly requeued: number;
			readonly skippedNotFailed: number;
			readonly deliveryIds: readonly string[];
			readonly window: FlowReplayWindow;
	  };

const REBUILD_FIELDS = ['replayed', 'skipped_already_delivered', 'rebuilt_delivery_ids'] as const;
const REQUEUE_FIELDS = ['requeued', 'skipped_not_failed', 'requeued_delivery_ids'] as const;

function isCount(value: unknown): value is number {
	return typeof value === 'number' && Number.isInteger(value) && value >= 0;
}

function isIdList(value: unknown): value is string[] {
	return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function isWindow(value: unknown): value is FlowReplayWindow {
	const window = value as Partial<FlowReplayWindow> | null;
	return (
		typeof window === 'object' &&
		window !== null &&
		typeof window.from === 'string' &&
		typeof window.to === 'string'
	);
}

/**
 * The wire `ReplayResult` is untagged, so the mode comes from the request. Each mode is parsed
 * from its own fields only; a response carrying the other mode's fields (or missing its own) is
 * a contract violation and yields `null` (`events-v1.md`: 混用字段即违约).
 */
export function parseReplayResponse(
	mode: FlowReplayMode,
	data: FlowReplayResponse | Record<string, unknown>
): ReplayOutcomeView | null {
	const raw = data as Record<string, unknown>;
	const own = mode === 'rebuild' ? REBUILD_FIELDS : REQUEUE_FIELDS;
	const other = mode === 'rebuild' ? REQUEUE_FIELDS : REBUILD_FIELDS;
	if (other.some((field) => field in raw)) return null;
	if (!isCount(raw[own[0]]) || !isCount(raw[own[1]]) || !isIdList(raw[own[2]])) return null;
	if (!isWindow(raw.window)) return null;
	return mode === 'rebuild'
		? {
				mode,
				replayed: raw.replayed as number,
				skippedAlreadyDelivered: raw.skipped_already_delivered as number,
				deliveryIds: raw.rebuilt_delivery_ids as string[],
				window: raw.window
			}
		: {
				mode,
				requeued: raw.requeued as number,
				skippedNotFailed: raw.skipped_not_failed as number,
				deliveryIds: raw.requeued_delivery_ids as string[],
				window: raw.window
			};
}

export interface ReplayDeps {
	readonly commands: Pick<FlowCommandService, 'replayDeliveries'>;
	readonly newKey: () => string;
	readonly now: () => number;
}

export interface ReplaySnapshot {
	readonly form: ReplayForm;
	readonly validationKey: string | null;
	readonly preview: ReplayOutcomeView | null;
	readonly executed: ReplayOutcomeView | null;
	readonly acknowledged: boolean;
	readonly busy: boolean;
	readonly failure: OpsFailure | null;
	readonly retry: 'dry_run' | 'execute' | null;
	readonly closed: boolean;
	readonly canExecute: boolean;
}

export type ReplayStepOutcome =
	| { readonly status: 'ok'; readonly outcome: ReplayOutcomeView }
	| { readonly status: 'invalid'; readonly errorKey: string }
	| { readonly status: 'refused' }
	| { readonly status: 'failed'; readonly failure: OpsFailure };

export function defaultReplayForm(now: number): ReplayForm {
	return {
		mode: 'rebuild',
		eventType: '',
		subscriberKind: '',
		subscriberId: '',
		from: toLocalInput(now - 3_600_000),
		to: toLocalInput(now - (now % 60_000))
	};
}

/** `datetime-local` value (browser local time, minute precision). */
export function toLocalInput(ms: number): string {
	const date = new Date(ms);
	const pad = (value: number) => String(value).padStart(2, '0');
	return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** Delivery replay: dry-run summary, explicit acknowledgement, then execute of the exact request
 * the dry-run described. */
export class DeliveryReplay {
	private form: ReplayForm;
	private preview: { outcome: ReplayOutcomeView; request: ReplayRequest } | null = null;
	private executed: ReplayOutcomeView | null = null;
	private acknowledged = false;
	private dryRunIntent: Intent | null = null;
	private executeIntent: Intent | null = null;
	private failure: OpsFailure | null = null;
	private retry: 'dry_run' | 'execute' | null = null;
	private validationKey: string | null = null;
	private busy = false;
	private closed = false;

	constructor(
		private readonly workspaceId: string,
		private readonly deps: ReplayDeps,
		initial?: ReplayForm
	) {
		this.form = initial ?? defaultReplayForm(deps.now());
	}

	/** Any change to the form voids the dry-run summary and the acknowledgement. */
	setForm(form: ReplayForm): void {
		if (JSON.stringify(form) === JSON.stringify(this.form)) return;
		this.form = form;
		this.preview = null;
		this.executed = null;
		this.acknowledged = false;
		this.dryRunIntent = null;
		this.executeIntent = null;
		this.failure = null;
		this.retry = null;
		this.validationKey = null;
		this.closed = false;
	}

	acknowledge(value: boolean): void {
		this.acknowledged = value;
	}

	canExecute(): boolean {
		if (this.busy || this.closed || this.executed !== null) return false;
		if (this.preview === null || !this.acknowledged) return false;
		return validateReplayForm(this.form, this.deps.now()).ok;
	}

	snapshot(): ReplaySnapshot {
		return {
			form: this.form,
			validationKey: this.validationKey,
			preview: this.preview?.outcome ?? null,
			executed: this.executed,
			acknowledged: this.acknowledged,
			busy: this.busy,
			failure: this.failure,
			retry: this.retry,
			closed: this.closed,
			canExecute: this.canExecute()
		};
	}

	async dryRun(): Promise<ReplayStepOutcome> {
		if (this.busy || this.closed || this.executed !== null) return { status: 'refused' };
		const validation = validateReplayForm(this.form, this.deps.now());
		if (!validation.ok) {
			this.validationKey = validation.errorKey;
			return { status: 'invalid', errorKey: validation.errorKey };
		}
		this.validationKey = null;
		const request = validation.request;
		const fingerprint = JSON.stringify(request);
		if (!this.dryRunIntent || this.dryRunIntent.fingerprint !== fingerprint) {
			this.dryRunIntent = { key: this.deps.newKey(), fingerprint };
		}
		const step = await this.send(request, true, this.dryRunIntent.key, 'dry_run');
		if (step.status === 'ok') {
			this.dryRunIntent = null;
			this.preview = { outcome: step.outcome, request };
			this.acknowledged = false;
		} else if (step.status === 'failed' && !step.failure.retryable) {
			this.dryRunIntent = null;
		}
		return step;
	}

	async execute(): Promise<ReplayStepOutcome> {
		if (!this.canExecute()) return { status: 'refused' };
		const preview = this.preview as { outcome: ReplayOutcomeView; request: ReplayRequest };
		const fingerprint = JSON.stringify(preview.request);
		if (!this.executeIntent || this.executeIntent.fingerprint !== fingerprint) {
			this.executeIntent = { key: this.deps.newKey(), fingerprint };
		}
		const step = await this.send(preview.request, false, this.executeIntent.key, 'execute');
		if (step.status === 'ok') {
			this.executeIntent = null;
			this.executed = step.outcome;
		} else if (step.status === 'failed' && !step.failure.retryable) {
			this.executeIntent = null;
			this.preview = null;
			this.acknowledged = false;
		}
		return step;
	}

	private async send(
		request: ReplayRequest,
		dryRun: boolean,
		key: string,
		step: 'dry_run' | 'execute'
	): Promise<ReplayStepOutcome> {
		this.busy = true;
		this.failure = null;
		this.retry = null;
		let result: ApiResult<FlowReplayResponse>;
		try {
			result = await this.deps.commands.replayDeliveries(this.workspaceId, {
				...request,
				dry_run: dryRun,
				idempotency_key: key
			});
		} catch {
			result = { code: 503, message: '', data: null };
		}
		this.busy = false;
		if (result.code === 0 && result.data) {
			const outcome = parseReplayResponse(request.mode, result.data);
			if (outcome) return { status: 'ok', outcome };
			return this.fail(failed('flow.operations.replay.error.malformed', false, true), step);
		}
		return this.fail(classifyOperationsError(result), step);
	}

	private fail(failure: OpsFailure, step: 'dry_run' | 'execute'): ReplayStepOutcome {
		this.failure = failure;
		if (failure.permanent) this.closed = true;
		this.retry = failure.retryable && !failure.permanent ? step : null;
		return { status: 'failed', failure };
	}
}

// ---- lag paging -------------------------------------------------------------------------------

/** Cursor pager over `GET .../flow/projection-lag` items (the admin lag endpoint returns no
 * per-object items at this baseline). */
export class ProjectionLagPager {
	private readonly cursors: Array<string | undefined> = [undefined];
	private index = 0;
	private page: FlowProjectionLag | null = null;

	constructor(
		private readonly workspaceId: string,
		private readonly reads: Pick<FlowObjectRepository, 'getProjectionLag'>,
		private readonly limit = OPS_LAG_PAGE_SIZE
	) {}

	get current(): FlowProjectionLag | null {
		return this.page;
	}

	get pageNumber(): number {
		return this.index + 1;
	}

	hasNext(): boolean {
		return typeof this.page?.next_cursor === 'string' && this.page.next_cursor !== '';
	}

	hasPrevious(): boolean {
		return this.index > 0;
	}

	load(): Promise<OpsRead<FlowProjectionLag>> {
		return this.fetch(this.index);
	}

	async next(): Promise<OpsRead<FlowProjectionLag> | null> {
		if (!this.hasNext()) return null;
		this.cursors[this.index + 1] = this.page?.next_cursor;
		return this.fetch(this.index + 1);
	}

	async previous(): Promise<OpsRead<FlowProjectionLag> | null> {
		if (!this.hasPrevious()) return null;
		return this.fetch(this.index - 1);
	}

	private async fetch(index: number): Promise<OpsRead<FlowProjectionLag>> {
		const read = toRead(
			await this.reads.getProjectionLag(this.workspaceId, {
				cursor: this.cursors[index],
				limit: this.limit
			})
		);
		if (read.status === 'ready') {
			this.index = index;
			this.page = read.data;
		}
		return read;
	}
}

// ---- panel composition ------------------------------------------------------------------------

export interface FlowOperationsDeps {
	readonly reads: Pick<
		FlowObjectRepository,
		'getAdminHealth' | 'getAdminLag' | 'getAdminIntegrity' | 'getProjectionLag'
	>;
	readonly commands: Pick<
		FlowCommandService,
		'verifyDocument' | 'compactDocument' | 'rebuildProjection' | 'replayDeliveries'
	>;
	readonly isWorkspaceAdmin: (workspaceId: string) => Promise<boolean>;
	readonly newKey: () => string;
	readonly now: () => number;
	readonly schedule: Scheduler;
}

export class FlowOperationsService {
	readonly deps: FlowOperationsDeps;

	constructor(
		readonly workspaceId: string,
		deps: Partial<FlowOperationsDeps> = {}
	) {
		this.deps = {
			reads: deps.reads ?? new FlowObjectRepository(),
			commands: deps.commands ?? new FlowCommandService(),
			isWorkspaceAdmin: deps.isWorkspaceAdmin ?? resolveWorkspaceAdmin,
			newKey: deps.newKey ?? newIdempotencyKey,
			now: deps.now ?? Date.now,
			schedule: deps.schedule ?? defaultScheduler
		};
	}

	/** Hides the panel for non-admins without calling any admin endpoint; the server's
	 * `require_flow_workspace_admin_access` is the real guard. */
	isWorkspaceAdmin(): Promise<boolean> {
		return this.deps.isWorkspaceAdmin(this.workspaceId);
	}

	healthMonitor(onChange: (snapshot: HealthSnapshot) => void): HealthMonitor {
		return new HealthMonitor(
			this.workspaceId,
			this.deps.reads,
			onChange,
			this.deps.schedule,
			this.deps.now
		);
	}

	async loadLag(): Promise<OpsRead<FlowAdminLag>> {
		return toRead(await this.deps.reads.getAdminLag(this.workspaceId));
	}

	projectionLagPager(): ProjectionLagPager {
		return new ProjectionLagPager(this.workspaceId, this.deps.reads);
	}

	async loadIntegrity(limit: OpsIntegrityLimit): Promise<OpsRead<FlowAdminIntegrity>> {
		return toRead(
			await this.deps.reads.getAdminIntegrity(this.workspaceId, { scope: 'documents', limit })
		);
	}

	operation(
		kind: MaintenanceKind,
		targetId: string,
		initialExpectedHead: number | null = null
	): MaintenanceOperation {
		return new MaintenanceOperation(
			kind,
			targetId.trim(),
			{ commands: this.deps.commands, newKey: this.deps.newKey },
			initialExpectedHead
		);
	}

	replay(): DeliveryReplay {
		return new DeliveryReplay(this.workspaceId, {
			commands: this.deps.commands,
			newKey: this.deps.newKey,
			now: this.deps.now
		});
	}
}

// ---- page session -----------------------------------------------------------------------------

export type OpsPanelView = 'loading' | 'ready' | 'forbidden';

/** Shown in the lag section when the admin endpoints answered but the per-object
 * `GET .../flow/projection-lag` was refused. That endpoint is gated on Flow being enabled (the
 * three `/admin/.../flow/*` endpoints are not), so with Flow off it is refused while the panel
 * itself is fully usable. */
export const PROJECTION_UNAVAILABLE_KEY = 'flow.operations.lag.projectionUnavailable';

export interface ProjectionBlock {
	/** The page of per-object rows on screen; kept across a failed page turn. */
	readonly data: FlowProjectionLag | null;
	/** A failure to show as an error (retry by paging or reloading); `''` when none. */
	readonly errorKey: string;
	/** The endpoint refused this viewer/workspace (Flow off): show `PROJECTION_UNAVAILABLE_KEY`
	 * as a notice, not an error, and no rows. */
	readonly unavailable: boolean;
}

/**
 * The lag section's own reading of a projection-lag result. It never decides the page view: the
 * page is forbidden only when the admin check or one of the three admin endpoints says so. A
 * refusal (legacy 403/404 or typed forbidden/not_found/feature_disabled) is the "Flow is off, no
 * per-object lag" notice; any other failure keeps the rows already shown and its own key.
 * `message` is never read.
 */
export function projectionBlockFromRead(
	read: OpsRead<FlowProjectionLag>,
	previous: FlowProjectionLag | null = null
): ProjectionBlock {
	if (read.status === 'ready') return { data: read.data, errorKey: '', unavailable: false };
	if (read.status === 'forbidden' || read.failure.messageKey === 'flow.error.feature_disabled') {
		return { data: null, errorKey: '', unavailable: true };
	}
	return { data: previous, errorKey: read.failure.messageKey, unavailable: false };
}

export interface OpsPanelState {
	readonly view: OpsPanelView;
	readonly health: HealthSnapshot | null;
	readonly lag: FlowAdminLag | null;
	readonly lagErrorKey: string;
	readonly projection: ProjectionBlock;
	readonly lagPage: number;
	readonly lagHasPrevious: boolean;
	readonly lagHasNext: boolean;
	readonly lagBusy: boolean;
	readonly integrity: FlowAdminIntegrity | null;
	readonly integrityErrorKey: string;
	readonly integrityLimit: OpsIntegrityLimit;
	readonly integrityBusy: boolean;
}

/**
 * Everything the operations page loads, owned outside the component so the mount/unmount race
 * and the view decision are unit-testable.
 *
 * - `open()` checks admin, reads health once, and only then starts the 15 s health poll and the
 *   section loads. If `dispose()` ran during any of those awaits (the user left the page), it
 *   stops there: no poll is started, nothing is emitted, and a monitor created in the meantime is
 *   disposed.
 * - The page view is `forbidden` only from the admin check or the three admin endpoints (health,
 *   lag, integrity). The projection-lag section reports its own failure (`ProjectionBlock`).
 */
export class OperationsPanelSession {
	private state: OpsPanelState = {
		view: 'loading',
		health: null,
		lag: null,
		lagErrorKey: '',
		projection: { data: null, errorKey: '', unavailable: false },
		lagPage: 1,
		lagHasPrevious: false,
		lagHasNext: false,
		lagBusy: false,
		integrity: null,
		integrityErrorKey: '',
		integrityLimit: OPS_INTEGRITY_LIMITS[0],
		integrityBusy: false
	};
	private monitor: HealthMonitor | null = null;
	private pager: ProjectionLagPager | null = null;
	private disposed = false;

	constructor(
		private readonly service: FlowOperationsService,
		private readonly onChange: (state: OpsPanelState) => void
	) {}

	get snapshot(): OpsPanelState {
		return this.state;
	}

	get isDisposed(): boolean {
		return this.disposed;
	}

	async open(): Promise<OpsPanelState> {
		const admin = await this.service.isWorkspaceAdmin();
		if (this.disposed) return this.state;
		if (!admin) {
			this.set({ view: 'forbidden' });
			return this.state;
		}
		const monitor = this.service.healthMonitor((health) => {
			if (this.disposed) return;
			this.set(health.forbidden ? { health, view: 'forbidden' } : { health });
		});
		this.monitor = monitor;
		const first = await monitor.refresh();
		if (this.disposed) {
			monitor.dispose();
			return this.state;
		}
		if (first.forbidden) return this.state;
		this.set({ view: 'ready' });
		monitor.start();
		this.pager = this.service.projectionLagPager();
		await Promise.all([this.loadLag(), this.loadProjection('load'), this.loadIntegrity()]);
		return this.state;
	}

	dispose(): void {
		this.disposed = true;
		this.monitor?.dispose();
		this.monitor = null;
	}

	pauseHealth(): void {
		this.monitor?.pause();
	}

	resumeHealth(): void {
		if (!this.disposed) this.monitor?.resume();
	}

	refreshHealth(): void {
		if (!this.disposed) void this.monitor?.refresh();
	}

	async loadLag(): Promise<void> {
		const read = await this.service.loadLag();
		if (this.disposed) return;
		if (read.status === 'ready') this.set({ lag: read.data, lagErrorKey: '' });
		else if (read.status === 'forbidden') this.set({ view: 'forbidden' });
		else this.set({ lagErrorKey: read.failure.messageKey });
	}

	async loadProjection(step: 'load' | 'previous' | 'next'): Promise<void> {
		const pager = this.pager;
		if (!pager || this.disposed) return;
		this.set({ lagBusy: true });
		const read =
			step === 'load'
				? await pager.load()
				: step === 'previous'
					? await pager.previous()
					: await pager.next();
		if (this.disposed) return;
		this.set({
			lagBusy: false,
			...(read ? { projection: projectionBlockFromRead(read, this.state.projection.data) } : {}),
			lagPage: pager.pageNumber,
			lagHasPrevious: pager.hasPrevious(),
			lagHasNext: pager.hasNext()
		});
	}

	async loadIntegrity(limit: OpsIntegrityLimit = this.state.integrityLimit): Promise<void> {
		if (this.disposed) return;
		this.set({ integrityLimit: limit, integrityBusy: true });
		const read = await this.service.loadIntegrity(limit);
		if (this.disposed) return;
		if (read.status === 'ready') {
			this.set({ integrity: read.data, integrityErrorKey: '', integrityBusy: false });
		} else if (read.status === 'forbidden') {
			this.set({ view: 'forbidden', integrityBusy: false });
		} else {
			this.set({ integrityErrorKey: read.failure.messageKey, integrityBusy: false });
		}
	}

	/** After a maintenance execute: reload every section and the health sample. */
	reloadAll(): void {
		if (this.disposed) return;
		void this.loadIntegrity();
		void this.loadLag();
		void this.loadProjection('load');
		void this.monitor?.refresh();
	}

	private set(patch: Partial<OpsPanelState>): void {
		this.state = { ...this.state, ...patch };
		this.onChange(this.state);
	}
}
