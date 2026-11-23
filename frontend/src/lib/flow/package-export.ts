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
// - a `failed` job is a failure even when its envelope is a success: a render that fails inside
//   the POST answers `code:0, status:"failed", error:<stable code>` (`rest-api-v1.md` v0.8,
//   2026-10-10), so the job's own `error` code selects the text, never only `error_code`;
// - the export write goes through `CommandService.startWorkspaceExport`, the job read and the
//   artifact download through `ObjectRepository` (`ui-surface-v1.md` five adapters);
// - recent job ids are a per-user, per-workspace convenience in `localStorage` (at most 10; every
//   read and write is guarded). The server is the source of truth.

import { sha256Hex } from '#lib/utils/sha256.js';
import { randomUuid } from '#lib/utils/uuid.js';
import type { ApiResult } from '#lib/api/client.js';
import type { CreateWorkspaceExportInput, FlowExportJob } from '#lib/api/flow.js';
import { projectsApi } from '#lib/api/projects.js';
import { FlowCommandService } from './command-service';
import { FlowObjectRepository } from './object-repository';
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

/** The failure a job reports about itself: a `failed` job's stored stable `error` code, worded
 * like the same code in an error envelope. A failed job without a code still fails (generic
 * text). Any other status is not a failure. */
export function exportJobFailure(
	job: Pick<FlowExportJob, 'status' | 'error'> | null
): FlowPackageFailure | null {
	if (!job || job.status !== 'failed') return null;
	const code = typeof job.error === 'string' && job.error !== '' ? job.error : null;
	if (code === null) return classifyPackageFailure({ code: 400, error_code: 'unknown' }, 'export');
	// The numeric code is irrelevant for a stable code; it only routes unknown codes to the
	// generic permanent text.
	return { ...classifyPackageFailure({ code: 400, error_code: code }, 'export'), retryable: false };
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
	readonly objectTypes?: readonly import('../api/flow').FlowPackageObjectType[];
	/** `null` exports the whole workspace. */
	readonly projectId: string | null;
}

/** Canonical explicit filter; unsupported kinds are rejected, never silently discarded. */
export function selectedPackageObjectTypes(
	types: readonly string[] | undefined
): import('../api/flow').FlowPackageObjectType[] | undefined {
	if (types === undefined) return undefined;
	if (
		!Array.isArray(types) ||
		types.length === 0 ||
		types.some((type) => type !== 'page' && type !== 'navigator')
	)
		throw new Error('invalid_package_object_types');
	return (['page', 'navigator'] as const).filter((type) => types.includes(type));
}

export function buildWorkspaceExportRequest(
	options: ExportOptions,
	idempotencyKey: string
): CreateWorkspaceExportInput {
	return {
		format: 'package',
		include_history: options.includeHistory,
		...(options.objectTypes !== undefined
			? { object_types: selectedPackageObjectTypes(options.objectTypes) }
			: {}),
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

export function recentExportsKey(scopeId: string): string {
	return `sylvode.flow.exports.${scopeId}`;
}

const JOB_ID = /^[A-Za-z0-9-]{1,64}$/;

export function readRecentExports(scopeId: string, store = browserStore()): string[] {
	if (!store) return [];
	try {
		const parsed: unknown = JSON.parse(store.getItem(recentExportsKey(scopeId)) ?? '[]');
		if (!Array.isArray(parsed)) return [];
		return parsed
			.filter((id): id is string => typeof id === 'string' && JOB_ID.test(id))
			.slice(0, RECENT_EXPORTS_MAX);
	} catch {
		return [];
	}
}

export function rememberExport(scopeId: string, jobId: string, store = browserStore()): string[] {
	const next = [jobId, ...readRecentExports(scopeId, store).filter((id) => id !== jobId)].slice(
		0,
		RECENT_EXPORTS_MAX
	);
	if (!store) return next;
	try {
		store.setItem(recentExportsKey(scopeId), JSON.stringify(next));
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
	readonly commands: Pick<FlowCommandService, 'startWorkspaceExport'>;
	readonly reads: Pick<FlowObjectRepository, 'getExportJob'>;
	readonly sleep: (ms: number) => Promise<void>;
	readonly newKey: () => string;
	readonly store: KeyValueStore | null;
}

function defaultSleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
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
		private readonly scopeId: string,
		private readonly onChange: (jobs: ExportJobView[]) => void = () => {},
		deps: Partial<PackageExportDeps> = {}
	) {
		this.deps = {
			commands: deps.commands ?? new FlowCommandService(),
			reads: deps.reads ?? new FlowObjectRepository(),
			sleep: deps.sleep ?? defaultSleep,
			newKey: deps.newKey ?? randomUuid,
			store: deps.store === undefined ? browserStore() : deps.store
		};
	}

	/** Tracked jobs, newest first. */
	get snapshot(): ExportJobView[] {
		return [...this.jobs.values()];
	}

	/** The idempotency key the next submit of `options` will send. Same options, same key. */
	keyFor(options: ExportOptions): string {
		const signature = JSON.stringify([
			options.includeHistory,
			options.projectId ?? null,
			selectedPackageObjectTypes(options.objectTypes) ?? null
		]);
		if (!this.intent || this.intent.signature !== signature) {
			this.intent = { key: this.deps.newKey(), signature };
		}
		return this.intent.key;
	}

	async submit(options: ExportOptions): Promise<ExportSubmitOutcome> {
		const key = this.keyFor(options);
		const result = await this.deps.commands.startWorkspaceExport(
			this.scopeId,
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
		rememberExport(this.scopeId, jobId, this.deps.store);
		const failed = exportJobFailure(result.data);
		if (failed) {
			// Already terminal: the job is stored (a reload re-attaches to it), nothing to poll.
			this.put({ jobId, job: result.data, polling: false, failure: failed }, true);
			return { status: 'failed', failure: failed };
		}
		this.put({ jobId, job: result.data, polling: true, failure: null }, true);
		void this.poll(jobId);
		return { status: 'started', jobId };
	}

	/** Re-attaches to the job ids remembered for this workspace and polls each to terminal. */
	resume(): void {
		for (const jobId of readRecentExports(this.scopeId, this.deps.store)) {
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
				const result: ApiResult<FlowExportJob> = await this.deps.reads.getExportJob(jobId);
				if (this.disposed) break;
				if (result.code === 0 && result.data) {
					const terminal = isTerminalExportStatus(result.data.status);
					this.put(
						{ jobId, job: result.data, polling: !terminal, failure: exportJobFailure(result.data) },
						false
					);
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

/** Lowercase hex SHA-256 of `blob`'s bytes, computed locally (see `#lib/utils/sha256.js`). */
export async function sha256HexOfBlob(blob: Blob): Promise<string> {
	return sha256Hex(new Uint8Array(await blob.arrayBuffer()));
}

/**
 * Fetches a finished job's artifact with the caller's credentials and hashes the downloaded
 * bytes locally (SHA-256). The local hash must equal the job's `checksum`; a missing checksum,
 * a different hash, or an `x-flow-package-sha256` header that disagrees with the checksum is
 * reported as `checksum_mismatch` and the bytes are not handed out. The header alone is never
 * accepted as proof of integrity.
 */
export async function downloadExport(
	job: FlowExportJob,
	fileName: string,
	reads: Pick<FlowObjectRepository, 'downloadExportArtifact'> = new FlowObjectRepository()
): Promise<ExportDownloadOutcome> {
	const url = exportDownloadUrl(job);
	if (!url) {
		return { status: 'failed', failure: classifyPackageFailure({ code: 404 }, 'report') };
	}
	const result = await reads.downloadExportArtifact(url);
	if (result.code !== 0 || !result.data) {
		return { status: 'failed', failure: classifyPackageFailure(result, 'report') };
	}
	const expected = typeof job.checksum === 'string' ? job.checksum.trim().toLowerCase() : '';
	const header = result.data.sha256?.trim().toLowerCase() ?? null;
	const local = expected ? await sha256HexOfBlob(result.data.blob) : null;
	if (!expected || local !== expected || (header !== null && header !== expected)) {
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

/** Page size for the project list (the projects endpoint caps `per_page` at 100). */
export const PROJECT_LIST_PAGE_SIZE = 100;
/** Safety bound on the pages walked (100 x 1000 projects); a workspace beyond it is a defect to
 * report, not something to truncate silently, so reaching it fails soft like any other error. */
export const PROJECT_LIST_MAX_PAGES = 1000;

/** Every project of the workspace, for the export scope, the import project mapping and the
 * Forms pickers: the list is read page by page (`page` 1..`total_pages`) so it is never cut at
 * the first 100. Fails soft to an empty list -- never to a partial one -- when any page cannot be
 * read (the whole-workspace / unprojected choices still work). */
export async function listWorkspaceProjects(
	workspaceId: string
): Promise<WorkspaceProjectOption[]> {
	const options: WorkspaceProjectOption[] = [];
	const seen = new Set<string>();
	try {
		for (let page = 1; page <= PROJECT_LIST_MAX_PAGES; page += 1) {
			const result = await projectsApi.list(workspaceId, {
				page,
				per_page: PROJECT_LIST_PAGE_SIZE
			});
			if (result.code !== 0 || !result.data) return [];
			for (const project of result.data.items) {
				if (seen.has(project.id)) continue;
				seen.add(project.id);
				options.push({
					id: project.id,
					label: project.key ? `${project.key} - ${project.name}` : project.name
				});
			}
			const totalPages = Number.isFinite(result.data.total_pages) ? result.data.total_pages : 1;
			if (page >= totalPages || result.data.items.length === 0) return options;
		}
		return [];
	} catch {
		return [];
	}
}
