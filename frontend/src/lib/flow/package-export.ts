// Workspace package export (`POST /workspaces/{id}/flow/exports` -> `GET /flow/exports/{job_id}`
// -> `download_url`), `contracts/rest-api-v1.md` v0.8 table; `surface-coverage-v1.md` rows
// "workspace export wizard" and "export status/download".
//
// Rules this module owns, so they are unit-testable without a component:
// - one export intent = one idempotency key; a retry of the same intent reuses it, a changed
//   option is a new intent with a new key;
// - after the POST the job is polled with `GET /flow/exports/{job_id}` -- immediately, then at
//   1s, 2s, 4s and 5s thereafter (exponential, capped at 5s) -- and polling stops at a terminal
//   status, on a permanent error, or when the tracker is disposed;
// - `download_url` is surfaced only for a successful terminal status;
// - recent job ids are a per-user, per-workspace convenience in `localStorage` (at most 10; every
//   read and write is guarded). The server is the source of truth.

import type { ApiResult } from '$lib/api/client';
import { flowApi, type CreateWorkspaceExportInput, type FlowExportJob } from '$lib/api/flow';
import { projectsApi } from '$lib/api/projects';
import { classifyPackageFailure, type FlowPackageFailure } from './package-import';

export type { FlowExportJob };

export const EXPORT_POLL_INITIAL_MS = 1_000;
export const EXPORT_POLL_MAX_MS = 5_000;
export const RECENT_EXPORTS_MAX = 10;

const SUCCESS_STATUSES: ReadonlySet<string> = new Set(['completed']);
const FAILURE_STATUSES: ReadonlySet<string> = new Set(['failed', 'expired', 'cancelled']);

/** i18n keys for the export statuses this client knows (static, so tests can check them). */
export const EXPORT_STATUS_KEYS: Readonly<Record<string, string>> = {
	queued: 'flow.export.status.queued',
	running: 'flow.export.status.running',
	completed: 'flow.export.status.completed',
	failed: 'flow.export.status.failed',
	expired: 'flow.export.status.expired',
	cancelled: 'flow.export.status.cancelled'
};

export function isTerminalExportStatus(status: string): boolean {
	return SUCCESS_STATUSES.has(status) || FAILURE_STATUSES.has(status);
}

export function isSuccessfulExportStatus(status: string): boolean {
	return SUCCESS_STATUSES.has(status);
}

/** The wait before poll number `attempt` (0-based) after the first, immediate one. */
export function exportPollDelay(attempt: number): number {
	return Math.min(EXPORT_POLL_MAX_MS, EXPORT_POLL_INITIAL_MS * 2 ** Math.max(0, attempt));
}

/** The download link for a job, or `null` unless the job finished successfully. */
export function exportDownloadUrl(job: FlowExportJob | null): string | null {
	if (!job || !isSuccessfulExportStatus(job.status)) return null;
	return typeof job.download_url === 'string' && job.download_url.length > 0
		? job.download_url
		: null;
}

/** `<workspace-slug-or-id>-<job id prefix>.sylvode-flow.zip`: the extension the import step
 * requires, so a downloaded package can be imported as is. */
export function exportFileName(workspaceLabel: string, jobId: string): string {
	const stem =
		workspaceLabel.replace(/[^A-Za-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'workspace';
	return `${stem}-${jobId.slice(0, 8)}.sylvode-flow.zip`;
}

export interface ExportOptions {
	readonly includeHistory: boolean;
	/** `null` exports the whole workspace. */
	readonly projectId: string | null;
}

export function buildWorkspaceExportRequest(
	options: ExportOptions,
	idempotencyKey: string
): CreateWorkspaceExportInput {
	return {
		format: 'package',
		include_history: options.includeHistory,
		...(options.projectId ? { project_id: options.projectId } : {}),
		idempotency_key: idempotencyKey
	};
}

/** Minimal `localStorage` surface, injectable for tests. */
export interface KeyValueStore {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
}

function browserStore(): KeyValueStore | null {
	try {
		return typeof localStorage === 'undefined' ? null : localStorage;
	} catch {
		return null;
	}
}

export function recentExportsKey(workspaceId: string): string {
	return `sylvode.flow.exports.${workspaceId}`;
}

const JOB_ID = /^[A-Za-z0-9-]{1,64}$/;

export function readRecentExports(workspaceId: string, store = browserStore()): string[] {
	if (!store) return [];
	try {
		const parsed: unknown = JSON.parse(store.getItem(recentExportsKey(workspaceId)) ?? '[]');
		if (!Array.isArray(parsed)) return [];
		return parsed
			.filter((id): id is string => typeof id === 'string' && JOB_ID.test(id))
			.slice(0, RECENT_EXPORTS_MAX);
	} catch {
		return [];
	}
}

export function rememberExport(
	workspaceId: string,
	jobId: string,
	store = browserStore()
): string[] {
	const next = [jobId, ...readRecentExports(workspaceId, store).filter((id) => id !== jobId)].slice(
		0,
		RECENT_EXPORTS_MAX
	);
	if (!store) return next;
	try {
		store.setItem(recentExportsKey(workspaceId), JSON.stringify(next));
	} catch {
		// Storage full or blocked: the list is a convenience only.
	}
	return next;
}

export interface ExportJobView {
	readonly jobId: string;
	readonly job: FlowExportJob | null;
	readonly polling: boolean;
	readonly failure: FlowPackageFailure | null;
}

export interface PackageExportDeps {
	readonly api: Pick<typeof flowApi, 'exportWorkspace' | 'getExportJob'>;
	readonly sleep: (ms: number) => Promise<void>;
	readonly newKey: () => string;
	readonly store: KeyValueStore | null;
}

function defaultSleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** One key per write intent; see `flow-settings.ts` for the non-secure-context fallback. */
export function newIdempotencyKey(): string {
	if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
	const bytes = crypto.getRandomValues(new Uint8Array(16));
	bytes[6] = (bytes[6] & 0x0f) | 0x40;
	bytes[8] = (bytes[8] & 0x3f) | 0x80;
	const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
	return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export type ExportSubmitOutcome =
	| { readonly status: 'started'; readonly jobId: string }
	| { readonly status: 'failed'; readonly failure: FlowPackageFailure };

/**
 * Export state machine for one workspace: submit -> poll -> terminal, plus re-attaching to
 * remembered jobs. `onChange` receives a fresh snapshot of every tracked job after each step.
 */
export class PackageExportController {
	private readonly deps: PackageExportDeps;
	private readonly jobs = new Map<string, ExportJobView>();
	private intent: { key: string; signature: string } | null = null;
	private disposed = false;
	private readonly polling = new Set<string>();

	constructor(
		private readonly workspaceId: string,
		private readonly onChange: (jobs: ExportJobView[]) => void = () => {},
		deps: Partial<PackageExportDeps> = {}
	) {
		this.deps = {
			api: deps.api ?? flowApi,
			sleep: deps.sleep ?? defaultSleep,
			newKey: deps.newKey ?? newIdempotencyKey,
			store: deps.store === undefined ? browserStore() : deps.store
		};
	}

	/** Tracked jobs, newest first. */
	get snapshot(): ExportJobView[] {
		return [...this.jobs.values()];
	}

	/** The idempotency key the next submit of `options` will send. Same options, same key. */
	keyFor(options: ExportOptions): string {
		const signature = JSON.stringify([options.includeHistory, options.projectId ?? null]);
		if (!this.intent || this.intent.signature !== signature) {
			this.intent = { key: this.deps.newKey(), signature };
		}
		return this.intent.key;
	}

	async submit(options: ExportOptions): Promise<ExportSubmitOutcome> {
		const key = this.keyFor(options);
		const result = await this.deps.api.exportWorkspace(
			this.workspaceId,
			buildWorkspaceExportRequest(options, key)
		);
		if (result.code !== 0 || !result.data) {
			const failure = classifyPackageFailure(result, 'export');
			// A retryable failure keeps the intent (and its key) for the retry.
			if (!failure.retryable) this.intent = null;
			return { status: 'failed', failure };
		}
		this.intent = null;
		const jobId = result.data.job_id;
		rememberExport(this.workspaceId, jobId, this.deps.store);
		this.put({ jobId, job: result.data, polling: true, failure: null }, true);
		void this.poll(jobId);
		return { status: 'started', jobId };
	}

	/** Re-attaches to the job ids remembered for this workspace and polls each to terminal. */
	resume(): void {
		for (const jobId of readRecentExports(this.workspaceId, this.deps.store)) {
			if (this.jobs.has(jobId)) continue;
			this.put({ jobId, job: null, polling: true, failure: null }, false);
			void this.poll(jobId);
		}
	}

	dispose(): void {
		this.disposed = true;
	}

	/** Polls one job until a terminal status, a permanent error, or `dispose`. */
	async poll(jobId: string): Promise<ExportJobView | null> {
		if (this.polling.has(jobId)) return this.jobs.get(jobId) ?? null;
		this.polling.add(jobId);
		try {
			for (let attempt = 0; !this.disposed; attempt += 1) {
				if (attempt > 0) await this.deps.sleep(exportPollDelay(attempt - 1));
				if (this.disposed) break;
				const result: ApiResult<FlowExportJob> = await this.deps.api.getExportJob(jobId);
				if (this.disposed) break;
				if (result.code === 0 && result.data) {
					const terminal = isTerminalExportStatus(result.data.status);
					this.put({ jobId, job: result.data, polling: !terminal, failure: null }, false);
					if (terminal) break;
					continue;
				}
				const failure = classifyPackageFailure(result, 'report');
				const previous = this.jobs.get(jobId)?.job ?? null;
				this.put({ jobId, job: previous, polling: failure.retryable, failure }, false);
				if (!failure.retryable) break;
			}
		} finally {
			this.polling.delete(jobId);
		}
		return this.jobs.get(jobId) ?? null;
	}

	private put(view: ExportJobView, newest: boolean): void {
		if (newest) {
			const rest = [...this.jobs.entries()].filter(([id]) => id !== view.jobId);
			this.jobs.clear();
			this.jobs.set(view.jobId, view);
			for (const [id, value] of rest) this.jobs.set(id, value);
		} else {
			this.jobs.set(view.jobId, view);
		}
		this.onChange(this.snapshot);
	}
}

export type ExportDownloadOutcome =
	| { readonly status: 'ready'; readonly blob: Blob; readonly fileName: string }
	| { readonly status: 'failed'; readonly failure: FlowPackageFailure };

/**
 * Fetches a finished job's artifact with the caller's credentials and checks the server's
 * `x-flow-package-sha256` header against the job's `checksum`; a mismatch is reported as
 * `checksum_mismatch` and the bytes are not handed out.
 */
export async function downloadExport(
	job: FlowExportJob,
	fileName: string,
	api: Pick<typeof flowApi, 'downloadExportArtifact'> = flowApi
): Promise<ExportDownloadOutcome> {
	const url = exportDownloadUrl(job);
	if (!url) {
		return { status: 'failed', failure: classifyPackageFailure({ code: 404 }, 'report') };
	}
	const result = await api.downloadExportArtifact(url);
	if (result.code !== 0 || !result.data) {
		return { status: 'failed', failure: classifyPackageFailure(result, 'report') };
	}
	if (result.data.sha256 !== null && result.data.sha256 !== job.checksum) {
		return {
			status: 'failed',
			failure: classifyPackageFailure({ code: 400, error_code: 'checksum_mismatch' }, 'report')
		};
	}
	return { status: 'ready', blob: result.data.blob, fileName };
}

export interface WorkspaceProjectOption {
	readonly id: string;
	readonly label: string;
}

/** Projects of the workspace, for the export scope and the import project mapping. Fails soft to
 * an empty list (the whole-workspace / unprojected choices still work). */
export async function listWorkspaceProjects(
	workspaceId: string
): Promise<WorkspaceProjectOption[]> {
	try {
		const result = await projectsApi.list(workspaceId, { page: 1, per_page: 100 });
		if (result.code !== 0 || !result.data) return [];
		return result.data.items.map((project) => ({
			id: project.id,
			label: project.key ? `${project.key} - ${project.name}` : project.name
		}));
	} catch {
		return [];
	}
}
