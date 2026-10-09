// Recoverable import report (`GET /workspaces/{id}/flow/imports/{import_id}` -> `ImportReport`,
// `contracts/export-package-v1.md` "Import report 与 lineage"). The report page is a deep link: it
// polls until a terminal status, and a reload simply starts polling again.
//
// The server answers a job that has not finished yet with a legacy `Conflict` (409, "import has
// not completed"), which is treated as "keep polling". A missing or foreign import (legacy
// `NotFound`/`Forbidden`) lands on one not-found-safe state so the page does not reveal which.

import type { ApiResult } from '$lib/api/client';
import { flowApi, type FlowPackageImportReport } from '$lib/api/flow';
import { exportPollDelay } from './package-export';
import { classifyPackageFailure, type FlowPackageFailure } from './package-import';

export type { FlowPackageImportReport };

export const IMPORT_REPORT_COUNT_KEYS = [
	'planned',
	'created',
	'reused',
	'detached',
	'failed'
] as const;

export const IMPORT_COUNT_LABEL_KEYS: Readonly<
	Record<(typeof IMPORT_REPORT_COUNT_KEYS)[number], string>
> = {
	planned: 'flow.import.report.count.planned',
	created: 'flow.import.report.count.created',
	reused: 'flow.import.report.count.reused',
	detached: 'flow.import.report.count.detached',
	failed: 'flow.import.report.count.failed'
};

export const IMPORT_STATUS_KEYS: Readonly<Record<string, string>> = {
	completed: 'flow.import.report.status.completed',
	failed: 'flow.import.report.status.failed'
};

const TERMINAL_IMPORT_STATUSES: ReadonlySet<string> = new Set(['completed', 'failed']);

export function isTerminalImportStatus(status: string): boolean {
	return TERMINAL_IMPORT_STATUSES.has(status);
}

export type ImportReportState =
	| { readonly status: 'loading' }
	| { readonly status: 'pending'; readonly report: FlowPackageImportReport | null }
	| { readonly status: 'ready'; readonly report: FlowPackageImportReport }
	| { readonly status: 'not_found' }
	| { readonly status: 'failed'; readonly failure: FlowPackageFailure };

export interface ImportReportDeps {
	readonly api: Pick<typeof flowApi, 'getPackageImport'>;
	readonly sleep: (ms: number) => Promise<void>;
}

export class ImportReportPoller {
	private readonly deps: ImportReportDeps;
	private disposed = false;

	constructor(
		private readonly workspaceId: string,
		private readonly importId: string,
		private readonly onChange: (state: ImportReportState) => void = () => {},
		deps: Partial<ImportReportDeps> = {}
	) {
		this.deps = {
			api: deps.api ?? flowApi,
			sleep: deps.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)))
		};
	}

	dispose(): void {
		this.disposed = true;
	}

	/** Polls until a terminal state or `dispose`; resolves with the last state. */
	async run(): Promise<ImportReportState> {
		let state: ImportReportState = { status: 'loading' };
		this.onChange(state);
		let last: FlowPackageImportReport | null = null;
		for (let attempt = 0; !this.disposed; attempt += 1) {
			if (attempt > 0) await this.deps.sleep(exportPollDelay(attempt - 1));
			if (this.disposed) break;
			const result: ApiResult<FlowPackageImportReport> = await this.deps.api.getPackageImport(
				this.workspaceId,
				this.importId
			);
			if (this.disposed) break;
			if (result.code === 0 && result.data) {
				last = result.data;
				state = isTerminalImportStatus(result.data.status)
					? { status: 'ready', report: result.data }
					: { status: 'pending', report: result.data };
				this.onChange(state);
				if (state.status === 'ready') break;
				continue;
			}
			const failure = classifyPackageFailure(result, 'report');
			const typedNotFound = result.error_code === 'not_found' || result.error_code === 'forbidden';
			const legacyNotFound =
				result.error_code === undefined && (result.code === 404 || result.code === 403);
			if (typedNotFound || legacyNotFound) {
				state = { status: 'not_found' };
				this.onChange(state);
				break;
			}
			if (failure.retryable) {
				state = { status: 'pending', report: last };
				this.onChange(state);
				continue;
			}
			state = { status: 'failed', failure };
			this.onChange(state);
			break;
		}
		return state;
	}
}
