// Flow -> Forms conversion job page (`/workspace/{id}/flow/conversions/{job_id}`; v0.7 Forms
// Bridge; `contracts/rest-api-v1.md` rows `GET /flow/conversions/{job_id}` and
// `POST /flow/conversions/{job_id}/retry`; `surface-coverage-v1.md` "Convert status" /
// "Convert retry", both `adapter:CommandService`).
//
// Backend facts this module is aligned with (`apps/api/src`):
// - the status set is exactly `started | completed | failed`
//   (`migrations/0062_flow_forms_bridge.sql:84` CHECK constraint; written by
//   `flow/bridge.rs:1309` `'started'`, `:1457` `'completed'`, `:1183-1184` `'failed'`);
// - terminal: `completed` and `failed`. `started` is the only non-terminal value;
// - retryable: `failed` only. `retry_conversion` replays a `completed` job or a repeated key and
//   answers a `started` job with `server_draining{contention}`; a real retry re-runs the
//   conversion with every permission re-checked against the frontier and target schema version
//   the JOB recorded -- it does not depend on the preview TTL (`rest-api-v1.md` v0.7, frozen
//   2026-10-10). A source that moved on answers `stale_frontier`; a target schema that changed
//   answers `policy_rejected` with `details.reason = "target_schema_changed"`;
// - `error` is the job's stored stable error code (one of the 14 in `error-mapping-v1.md`),
//   worded through `flow.error.<code>`, never driver text;
// - a retry of a job that is still `started` (another request is running it) answers
//   `server_draining` with `details.reason = "contention"` and `retry_after_ms`: worded "try again
//   in N s", retry stays available, and the page follows the job until it ends;
// - a job left `started` past the server's threshold is reaped to `failed` /
//   `error = "server_rejected"` server-side (at API start and periodically) and marked
//   `error_reason = "stale_started"` (`rest-api-v1.md` v0.7, 2026-10-10); only that reaped job is
//   an interrupted run rather than a permanent refusal, so it is worded "interrupted (reclaimed
//   by the server)" and offers retry. This page keeps polling a `started` job, so the reaped
//   status appears without a reload;
// - a stored `stale_frontier` is worded for this page (the source Page moved on since the job's
//   preview), not with the editor's generic "reload and replay" text.
//
// Rules this module owns, so they are unit-testable without a component:
// - the job is polled immediately, then after 1s, 2s, 4s and every 5s (`exportPollDelay`) until a
//   terminal status, a permanent error, or `dispose`; a reload simply starts polling again;
// - a transient read failure (5xx / network) keeps polling and keeps the last job on screen;
// - `not_found` and `forbidden` (typed or legacy 404/403) land on one safe empty state that does
//   not reveal whether the job exists; a malformed job id never reaches the server;
// - retry is offered only for a retryable failed job (`canRetry`), only after an explicit
//   confirmation (`requestRetry` -> `confirmRetry`), and with ONE idempotency key per job id that
//   every retry of that job reuses; a `server_rejected` job or rejection is permanent and never
//   offers retry; any other permanent retry rejection also withdraws the retry action;
// - a refused retry re-reads the job once, so its new stored `error` is shown;
// - errors branch on `error_code` first and the numeric code second, never on `message`.

import type { ApiResult } from '#lib/api/client.js';
import type { FlowConversionJob, FlowObjectView } from '#lib/api/flow.js';
import { formsApi, type FormRecord } from '#lib/api/forms.js';
import type { FlowCommandService } from './command-service';
import { POLICY_REJECTED_REASONS, type PolicyRejectedReason } from './convert-wizard';
import {
	FLOW_ERROR_CODES,
	flowErrorFromEnvelope,
	flowErrorI18nKey,
	parseRetryAfterMs,
	parseServerDrainingReason
} from './errors';
import type { FlowObjectRepository } from './object-repository';
import { exportPollDelay, newIdempotencyKey } from './package-export';
import type { FlowErrorCode } from './types';

export type { FlowConversionJob };

export type ConversionJobStatus = FlowConversionJob['status'];

/** Every status the backend can store (`0062_flow_forms_bridge.sql:84`). */
export const CONVERSION_JOB_STATUSES = [
	'started',
	'completed',
	'failed'
] as const satisfies readonly ConversionJobStatus[];

const TERMINAL_STATUSES: ReadonlySet<string> = new Set<ConversionJobStatus>([
	'completed',
	'failed'
]);
const RETRYABLE_STATUSES: ReadonlySet<string> = new Set<ConversionJobStatus>(['failed']);

export function isKnownConversionStatus(status: string): status is ConversionJobStatus {
	return (CONVERSION_JOB_STATUSES as readonly string[]).includes(status);
}

/** Whether polling stops at `status`. A status this client does not know also stops polling: it
 * is shown as unknown and offers no action, rather than being polled forever. */
export function isTerminalConversionStatus(status: string): boolean {
	return TERMINAL_STATUSES.has(status) || !isKnownConversionStatus(status);
}

/** Whether this failed job was reaped by the server (`error_reason = "stale_started"`): its run
 * was interrupted, not refused. */
export function isReapedJob(
	job: Pick<FlowConversionJob, 'status' | 'error_reason'> | null
): boolean {
	return job !== null && job.status === 'failed' && job.error_reason === 'stale_started';
}

/** Whether the server would accept a retry of this job: a `failed` job whose stored error is not
 * the permanent `server_rejected` (`error-mapping-v1.md`: "不得提示重试") -- unless the server
 * reaped it (`stale_started`), which is an interrupted run and retryable. */
export function canRetry(
	job: Pick<FlowConversionJob, 'status' | 'error' | 'error_reason'> | null
): boolean {
	if (job === null || !RETRYABLE_STATUSES.has(job.status)) return false;
	return job.error !== 'server_rejected' || isReapedJob(job);
}

// i18n keys, spelled out statically so the unit tests can check every one exists in zh and en.
export const JOB_STATUS_KEYS: Readonly<Record<ConversionJobStatus | 'unknown', string>> = {
	started: 'flow.bridge.job.status.started',
	completed: 'flow.bridge.job.status.completed',
	failed: 'flow.bridge.job.status.failed',
	unknown: 'flow.bridge.job.status.unknown'
};

export const JOB_TARGET_TYPE_KEYS: Readonly<
	Record<FlowConversionJob['target_type'] | 'unknown', string>
> = {
	form: 'flow.bridge.job.targetType.form',
	form_record: 'flow.bridge.job.targetType.formRecord',
	unknown: 'flow.bridge.job.targetType.unknown'
};

/** The wording for a job's `target_type`; a value this client does not know is "unknown". */
export function jobTargetTypeKey(targetType: unknown): string {
	return targetType === 'form' || targetType === 'form_record'
		? JOB_TARGET_TYPE_KEYS[targetType]
		: JOB_TARGET_TYPE_KEYS.unknown;
}

export function jobStatusKey(status: string): string {
	return isKnownConversionStatus(status) ? JOB_STATUS_KEYS[status] : JOB_STATUS_KEYS.unknown;
}

export const JOB_FAILURE_KEYS = {
	not_found: 'flow.bridge.job.error.notFound',
	unauthenticated: 'flow.error.unauthenticated',
	server_rejected: 'flow.error.server_rejected',
	stale_frontier: 'flow.bridge.job.error.staleFrontier',
	policy_rejected: 'flow.bridge.job.error.policyRejected',
	target_schema_changed: 'flow.bridge.job.error.targetSchemaChanged',
	permission_unconfigured: 'flow.bridge.job.error.permissionUnconfigured',
	rejected: 'flow.bridge.job.error.rejected',
	conflict: 'flow.bridge.job.error.conflict',
	unavailable: 'flow.bridge.job.error.unavailable',
	busy: 'flow.bridge.job.error.busy',
	jobFailed: 'flow.bridge.job.error.jobFailed',
	staleStarted: 'flow.bridge.job.error.staleStarted',
	staleSource: 'flow.bridge.job.staleSource'
} as const;

/** The text for a job's stored error. A reaped job (`error_reason = "stale_started"`) is
 * "interrupted (reclaimed by the server)"; a stored `stale_frontier` gets this page's own wording;
 * other known Flow codes use their shared key; anything else gets one generic sentence, so no
 * stored code is ever shown as prose. */
export function jobErrorKey(
	error: string | null,
	errorReason: string | null | undefined = null
): string | null {
	if (error === null || error === '') return null;
	if (errorReason === 'stale_started') return JOB_FAILURE_KEYS.staleStarted;
	if (error === 'server_rejected') return JOB_FAILURE_KEYS.server_rejected;
	if (error === 'stale_frontier') return JOB_FAILURE_KEYS.staleSource;
	if ((FLOW_ERROR_CODES as readonly string[]).includes(error)) {
		return flowErrorI18nKey({ code: error as FlowErrorCode, details: undefined });
	}
	return JOB_FAILURE_KEYS.jobFailed;
}

export type JobFailureKind =
	| 'not_found'
	| 'unauthenticated'
	| 'server_rejected'
	| 'stale_frontier'
	| 'policy_rejected'
	| 'rejected'
	| 'conflict'
	| 'unavailable'
	| 'contention'
	| 'flow_error';

export interface JobFailure {
	readonly kind: JobFailureKind;
	readonly messageKey: string;
	/** Whether sending the same request again (same key) may succeed. */
	readonly retryable: boolean;
	/** `policy_rejected.details.reason` when it is an approved conversion reason. */
	readonly reason: PolicyRejectedReason | null;
	/** `server_draining.details.retry_after_ms` of a `contention` answer (a lower bound). */
	readonly retryAfterMs?: number | null;
	/** Interpolation values for `messageKey`. */
	readonly messageValues?: Readonly<Record<string, number>>;
}

function failure(
	kind: JobFailureKind,
	messageKey: string,
	retryable: boolean,
	reason: PolicyRejectedReason | null = null
): JobFailure {
	return { kind, messageKey, retryable, reason };
}

/** `server_draining{reason:"contention"}`: another request holds this job right now (a retry of a
 * job that is still `started`, `rest-api-v1.md` v0.7 / `error-mapping-v1.md`). Not permanent and
 * not a read-only state: the user may try again after `retry_after_ms` (at least a second). */
function contentionFailure(details: unknown): JobFailure {
	const retryAfterMs = parseRetryAfterMs(details);
	const seconds = Math.max(1, Math.ceil((retryAfterMs ?? 0) / 1000));
	return {
		kind: 'contention',
		messageKey: JOB_FAILURE_KEYS.busy,
		retryable: true,
		reason: null,
		retryAfterMs,
		messageValues: { seconds }
	};
}

function policyReason(details: unknown): PolicyRejectedReason | null {
	if (!details || typeof details !== 'object') return null;
	const reason = (details as { reason?: unknown }).reason;
	return typeof reason === 'string' &&
		(POLICY_REJECTED_REASONS as readonly string[]).includes(reason)
		? (reason as PolicyRejectedReason)
		: null;
}

/**
 * Classifies a non-success envelope from the status read or the retry. `error_code` first, the
 * numeric code second, `message` never.
 *
 * `stale_frontier` and `policy_rejected` on retry are permanent for THIS job: the retry re-runs
 * against the frontier and target schema version the job recorded, so once the Page or the form
 * moved on the same request cannot succeed later. `policy_rejected` is worded by its
 * `details.reason` (`target_schema_changed`, `permission_unconfigured`); without an approved
 * reason it keeps the generic wording.
 */
export function classifyJobFailure(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): JobFailure {
	const flowError = flowErrorFromEnvelope(result);
	if (flowError) {
		switch (flowError.code) {
			case 'server_rejected':
				// Permanent (`error-mapping-v1.md`: "不得提示重试").
				return failure('server_rejected', JOB_FAILURE_KEYS.server_rejected, false);
			case 'forbidden':
			case 'not_found':
				return failure('not_found', JOB_FAILURE_KEYS.not_found, false);
			case 'unauthenticated':
				return failure('unauthenticated', JOB_FAILURE_KEYS.unauthenticated, false);
			case 'stale_frontier':
				return failure('stale_frontier', JOB_FAILURE_KEYS.stale_frontier, false);
			case 'server_draining':
				if (parseServerDrainingReason(result.details) === 'contention') {
					return contentionFailure(result.details);
				}
				return failure('flow_error', flowErrorI18nKey(flowError), flowError.recoverable);
			case 'policy_rejected': {
				const reason = policyReason(result.details);
				const key =
					reason === 'target_schema_changed'
						? JOB_FAILURE_KEYS.target_schema_changed
						: reason === 'permission_unconfigured'
							? JOB_FAILURE_KEYS.permission_unconfigured
							: JOB_FAILURE_KEYS.policy_rejected;
				return failure('policy_rejected', key, false, reason);
			}
			default:
				return failure('flow_error', flowErrorI18nKey(flowError), flowError.recoverable);
		}
	}
	if (typeof result.error_code === 'string' && result.error_code !== '') {
		// A typed code this client does not know: permanent, no retry offered.
		return failure('rejected', JOB_FAILURE_KEYS.rejected, false);
	}
	switch (result.code) {
		case 401:
			return failure('unauthenticated', JOB_FAILURE_KEYS.unauthenticated, false);
		case 403:
		case 404:
			return failure('not_found', JOB_FAILURE_KEYS.not_found, false);
		case 409:
			return failure('conflict', JOB_FAILURE_KEYS.conflict, false);
		default:
			return result.code >= 500
				? failure('unavailable', JOB_FAILURE_KEYS.unavailable, true)
				: failure('rejected', JOB_FAILURE_KEYS.rejected, false);
	}
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The API parses `{job_id}` as a UUID; anything else is answered outside the JSON envelope, so it
 * is treated as not found without a request. */
export function isConversionJobId(value: string): boolean {
	return UUID_RE.test(value);
}

export type JobLoadState = 'loading' | 'ready' | 'not_found' | 'failed';

/** The source object as the viewer may see it: its title, or only its id. */
export type SourceView =
	| { readonly state: 'loading' }
	| { readonly state: 'ready'; readonly title: string }
	| { readonly state: 'hidden' };

/** One created target: a Forms record this viewer can open, or just the id. */
export type TargetView =
	| { readonly id: string; readonly state: 'loading' }
	| {
			readonly id: string;
			readonly state: 'record';
			readonly projectId: string;
			readonly title: string;
	  }
	| { readonly id: string; readonly state: 'unresolved' };

export interface ConversionJobSnapshot {
	readonly jobId: string;
	readonly loadState: JobLoadState;
	readonly job: FlowConversionJob | null;
	readonly terminal: boolean;
	readonly polling: boolean;
	/** A permanent error before any job was read. */
	readonly loadFailure: JobFailure | null;
	/** The last status read failed; the shown job may be out of date. */
	readonly pollFailure: JobFailure | null;
	readonly source: SourceView;
	readonly targets: readonly TargetView[];
	/** Whether the retry action is offered right now. */
	readonly canRetry: boolean;
	readonly confirmingRetry: boolean;
	readonly retrying: boolean;
	readonly retryFailure: JobFailure | null;
}

export interface ConversionJobDeps {
	readonly commands: Pick<FlowCommandService, 'convertStatus' | 'convertRetry'>;
	readonly repository: Pick<FlowObjectRepository, 'get'>;
	readonly getRecord: (recordId: string) => Promise<ApiResult<FormRecord>>;
	readonly sleep: (ms: number) => Promise<void>;
	readonly newKey: () => string;
}

export type RetryOutcome =
	| { readonly status: 'refused' }
	| { readonly status: 'retried'; readonly job: FlowConversionJob }
	| { readonly status: 'failed'; readonly failure: JobFailure };

function defaultSleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Polls one conversion job, resolves its source and targets for display, and gates retry. */
export class ConversionJobController {
	private readonly deps: ConversionJobDeps;
	private job: FlowConversionJob | null = null;
	private loadState: JobLoadState = 'loading';
	private loadFailure: JobFailure | null = null;
	private pollFailure: JobFailure | null = null;
	private polling = false;
	private disposed = false;
	private source: SourceView = { state: 'loading' };
	private sourceFor: string | null = null;
	private targets: readonly TargetView[] = [];
	private targetsFor: string | null = null;
	/** One retry key per job id, minted on the first confirmed retry and reused by every later
	 * retry of this job. */
	private retryKey: string | null = null;
	private confirming = false;
	private retrying = false;
	private retryFailure: JobFailure | null = null;
	private retryWithdrawn = false;

	constructor(
		private readonly workspaceId: string,
		private readonly jobId: string,
		private readonly onChange: (snapshot: ConversionJobSnapshot) => void = () => {},
		deps: Partial<ConversionJobDeps> & Pick<ConversionJobDeps, 'commands' | 'repository'>
	) {
		this.deps = {
			commands: deps.commands,
			repository: deps.repository,
			getRecord: deps.getRecord ?? ((recordId) => formsApi.getRecord(recordId)),
			sleep: deps.sleep ?? defaultSleep,
			newKey: deps.newKey ?? newIdempotencyKey
		};
	}

	snapshot(): ConversionJobSnapshot {
		return {
			jobId: this.jobId,
			loadState: this.loadState,
			job: this.job,
			terminal: this.job !== null && isTerminalConversionStatus(this.job.status),
			polling: this.polling,
			loadFailure: this.loadFailure,
			pollFailure: this.pollFailure,
			source: this.source,
			targets: this.targets,
			canRetry: this.retryAvailable(),
			confirmingRetry: this.confirming,
			retrying: this.retrying,
			retryFailure: this.retryFailure
		};
	}

	dispose(): void {
		this.disposed = true;
	}

	/** Polls until a terminal status, a permanent error, or `dispose`; resolves with the load
	 * state. A second call while a poll loop is running does not start another one. */
	async poll(): Promise<JobLoadState> {
		if (this.polling || this.disposed) return this.loadState;
		if (!isConversionJobId(this.jobId)) {
			this.toNotFound();
			return this.loadState;
		}
		this.polling = true;
		this.emit();
		try {
			for (let attempt = 0; !this.disposed; attempt += 1) {
				if (attempt > 0) await this.deps.sleep(exportPollDelay(attempt - 1));
				if (this.disposed) break;
				const result = await this.read();
				if (this.disposed) break;
				if (result.code === 0 && result.data) {
					this.pollFailure = null;
					await this.applyJob(result.data);
					if (isTerminalConversionStatus(result.data.status)) break;
					continue;
				}
				const readFailure = classifyJobFailure(result);
				if (readFailure.kind === 'not_found') {
					this.toNotFound();
					break;
				}
				if (readFailure.retryable) {
					this.pollFailure = readFailure;
					this.emit();
					continue;
				}
				if (this.job) {
					this.pollFailure = readFailure;
				} else {
					this.loadState = 'failed';
					this.loadFailure = readFailure;
				}
				break;
			}
		} finally {
			this.polling = false;
			this.emit();
		}
		return this.loadState;
	}

	/** Opens the retry confirmation. Refused unless the job can be retried right now. */
	requestRetry(): boolean {
		if (!this.retryAvailable()) return false;
		this.confirming = true;
		this.retryFailure = null;
		this.emit();
		return true;
	}

	cancelRetry(): void {
		if (!this.confirming) return;
		this.confirming = false;
		this.emit();
	}

	/** Sends the retry after the user confirmed it. Without an open confirmation, or when the job
	 * is not retryable, nothing is sent. */
	async confirmRetry(): Promise<RetryOutcome> {
		if (!this.confirming || !this.retryAvailable()) return { status: 'refused' };
		this.confirming = false;
		this.retrying = true;
		this.retryFailure = null;
		this.emit();
		if (this.retryKey === null) this.retryKey = this.deps.newKey();
		let result: ApiResult<FlowConversionJob>;
		try {
			result = await this.deps.commands.convertRetry(this.jobId, this.retryKey);
		} catch {
			result = { code: 500, message: '', data: null };
		}
		this.retrying = false;
		if (result.code === 0 && result.data) {
			await this.applyJob(result.data);
			if (!isTerminalConversionStatus(result.data.status)) void this.poll();
			return { status: 'retried', job: result.data };
		}
		const retryFailure = classifyJobFailure(result);
		if (retryFailure.kind === 'not_found') {
			this.toNotFound();
			return { status: 'failed', failure: retryFailure };
		}
		if (!retryFailure.retryable) this.retryWithdrawn = true;
		this.retryFailure = retryFailure;
		this.emit();
		// A refused retry may have changed the stored job (e.g. a new `error` code): read it again.
		// A job another request is running (`contention`) is `started`: follow it to its end.
		const reread = await this.read();
		if (reread.code === 0 && reread.data && !this.disposed) {
			await this.applyJob(reread.data);
			if (!isTerminalConversionStatus(reread.data.status)) void this.poll();
		}
		return { status: 'failed', failure: retryFailure };
	}

	private retryAvailable(): boolean {
		return (
			this.loadState === 'ready' && !this.retryWithdrawn && !this.retrying && canRetry(this.job)
		);
	}

	private async read(): Promise<ApiResult<FlowConversionJob>> {
		try {
			return await this.deps.commands.convertStatus(this.jobId);
		} catch {
			return { code: 500, message: '', data: null };
		}
	}

	private toNotFound(): void {
		this.job = null;
		this.loadState = 'not_found';
		this.loadFailure = null;
		this.pollFailure = null;
		this.confirming = false;
		this.source = { state: 'hidden' };
		this.targets = [];
		this.emit();
	}

	private async applyJob(job: FlowConversionJob): Promise<void> {
		this.job = Object.freeze({
			...job,
			created_target_ids: [...job.created_target_ids],
			warnings: [...job.warnings]
		});
		this.loadState = 'ready';
		this.loadFailure = null;
		this.emit();
		await Promise.all([this.resolveSource(job.source_object_id), this.resolveTargets(job)]);
	}

	/** The source title through `ObjectRepository.get`; without access only the id is shown. */
	private async resolveSource(objectId: string): Promise<void> {
		if (this.sourceFor === objectId) return;
		this.sourceFor = objectId;
		this.source = { state: 'loading' };
		this.emit();
		let result: ApiResult<FlowObjectView> | null = null;
		try {
			result = await this.deps.repository.get(objectId);
		} catch {
			result = null;
		}
		if (this.sourceFor !== objectId) return;
		const object = result && result.code === 0 ? result.data : null;
		this.source =
			object && object.id === objectId && object.workspace_id === this.workspaceId
				? { state: 'ready', title: object.title }
				: { state: 'hidden' };
		this.emit();
	}

	/** Each created id is looked up as a Forms record; one that is not a record this viewer can
	 * read in this workspace stays a bare id. No link is ever built from a guess. */
	private async resolveTargets(job: FlowConversionJob): Promise<void> {
		const ids = [...job.created_target_ids];
		const signature = JSON.stringify(ids);
		if (this.targetsFor === signature) return;
		this.targetsFor = signature;
		this.targets = ids.map((id) => ({ id, state: 'loading' }) as const);
		this.emit();
		const resolved = await Promise.all(ids.map((id) => this.resolveTarget(id)));
		if (this.targetsFor !== signature) return;
		this.targets = resolved;
		this.emit();
	}

	private async resolveTarget(id: string): Promise<TargetView> {
		try {
			const result = await this.deps.getRecord(id);
			const record = result.code === 0 ? result.data : null;
			if (
				record &&
				record.id === id &&
				record.workspace_id === this.workspaceId &&
				typeof record.project_id === 'string' &&
				record.project_id !== ''
			) {
				return { id, state: 'record', projectId: record.project_id, title: record.title };
			}
		} catch {
			// Fall through to the bare id.
		}
		return { id, state: 'unresolved' };
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}
