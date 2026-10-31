// Single-object export (`POST /flow/objects/{object_id}/exports` -> `GET /flow/exports/{job_id}`
// -> `download_url`, `contracts/rest-api-v1.md` v0.8 table).
//
// The write goes through `CommandService.startObjectExport`; polling and the download reuse
// `package-export.ts`: `PackageExportController.poll` (immediate, then 1s/2s/4s/5s, stop at a
// terminal status, a permanent error or `dispose`) and `downloadExport` (local SHA-256 of the
// downloaded bytes must equal the job's `checksum`). One key per intent; a retry of the same
// options reuses it.
//
// Format rules from the server (`apps/api/src/flow/export.rs::validate_request`):
// `json|markdown|csv|package`, and `include_history` is only accepted with `package`. The panel
// offers `csv` only for a Collection.

import type {
	CreateObjectExportInput,
	FlowExportJob,
	FlowObjectExportFormat
} from '#lib/api/flow.js';
import { FlowCommandService } from './command-service';
import { FlowObjectRepository } from './object-repository';
import {
	PackageExportController,
	downloadExport,
	newIdempotencyKey,
	type ExportDownloadOutcome,
	type ExportJobView
} from './package-export';
import {
	classifyPackageFailure,
	type FlowPackageEnvelope,
	type FlowPackageFailure
} from './package-import';
import { FLOW_ERROR_CODES, flowErrorFromEnvelope, flowErrorI18nKey } from './errors';
import type { FlowErrorCode } from './types';

export const OBJECT_EXPORT_FORMATS = [
	'json',
	'markdown',
	'csv',
	'package'
] as const satisfies readonly FlowObjectExportFormat[];

export const OBJECT_EXPORT_FORMAT_KEYS: Readonly<Record<FlowObjectExportFormat, string>> = {
	json: 'flow.panel.export.format.json',
	markdown: 'flow.panel.export.format.markdown',
	csv: 'flow.panel.export.format.csv',
	package: 'flow.panel.export.format.package'
};

const EXTENSIONS: Readonly<Record<FlowObjectExportFormat, string>> = {
	json: 'json',
	markdown: 'md',
	csv: 'csv',
	package: 'sylvode-flow.zip'
};

/** `policy_rejected{details.reason:"field_secrecy"}`: the object is a Record with restricted
 * fields, and a package would carry their values (`rest-api-v1.md` v0.6 "导出与字段保密"). */
export const OBJECT_EXPORT_FIELD_SECRECY_KEY = 'flow.panel.export.fieldSecrecy';

/** Classifies a refused export start. The field secrecy refusal gets its own wording (export as
 * JSON or Markdown instead); everything else is the shared package classification. */
export function classifyObjectExportFailure(result: FlowPackageEnvelope): FlowPackageFailure {
	const flowError = flowErrorFromEnvelope(result);
	const reason =
		typeof result.details === 'object' && result.details !== null
			? (result.details as { reason?: unknown }).reason
			: undefined;
	if (flowError?.code === 'policy_rejected' && reason === 'field_secrecy') {
		return {
			messageKey: OBJECT_EXPORT_FIELD_SECRECY_KEY,
			values: {},
			blocking: false,
			retryable: false,
			forbidden: false
		};
	}
	return classifyPackageFailure(result, 'export');
}

/** The formats offered for an object of `objectType`. A Record keeps `package`: whether it has
 * restricted fields is the server's decision, answered as `field_secrecy` when it does. */
export function objectExportFormats(objectType: string): FlowObjectExportFormat[] {
	return OBJECT_EXPORT_FORMATS.filter((format) => format !== 'csv' || objectType === 'collection');
}

export interface ObjectExportOptions {
	readonly format: FlowObjectExportFormat;
	/** Only meaningful for `package`; ignored (sent as `false`) for every other format. */
	readonly includeHistory: boolean;
}

export function buildObjectExportRequest(
	options: ObjectExportOptions,
	idempotencyKey: string
): CreateObjectExportInput {
	return {
		format: options.format,
		include_history: options.format === 'package' && options.includeHistory,
		idempotency_key: idempotencyKey
	};
}

/** `<title-or-id>-<job id prefix>.<ext>`. */
export function objectExportFileName(
	title: string,
	jobId: string,
	format: FlowObjectExportFormat
): string {
	const stem = title.replace(/[^A-Za-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '') || 'page';
	return `${stem}-${jobId.slice(0, 8)}.${EXTENSIONS[format] ?? 'bin'}`;
}

/** i18n key for a `failed` job's stable `error` code (`rest-api-v1.md`: "`error?` 在 `failed` 终态
 * 必填，取稳定错误 code"), or `null` when the job did not fail. Unknown codes get a generic key. */
export function exportJobErrorKey(job: FlowExportJob | null): string | null {
	if (!job || job.status !== 'failed') return null;
	const code = job.error;
	if (typeof code === 'string' && (FLOW_ERROR_CODES as readonly string[]).includes(code)) {
		return flowErrorI18nKey({ code: code as FlowErrorCode });
	}
	return 'flow.panel.export.failed';
}

export interface ObjectExportDeps {
	readonly commands: Pick<FlowCommandService, 'startObjectExport'>;
	readonly reads: Pick<FlowObjectRepository, 'getExportJob' | 'downloadExportArtifact'>;
	readonly sleep: (ms: number) => Promise<void>;
	readonly newKey: () => string;
}

export type ObjectExportSubmitOutcome =
	| { readonly status: 'started'; readonly jobId: string }
	| { readonly status: 'refused' }
	| { readonly status: 'failed'; readonly failure: FlowPackageFailure };

export interface ObjectExportSnapshot {
	readonly job: ExportJobView | null;
	readonly format: FlowObjectExportFormat | null;
}

export class ObjectExportController {
	private readonly deps: ObjectExportDeps;
	private readonly poller: PackageExportController;
	private intent: { key: string; signature: string } | null = null;
	private jobId: string | null = null;
	private format: FlowObjectExportFormat | null = null;
	private submitting = false;

	constructor(
		private readonly objectId: string,
		workspaceId: string,
		private readonly onChange: (snapshot: ObjectExportSnapshot) => void = () => {},
		deps: Partial<ObjectExportDeps> = {}
	) {
		const reads = deps.reads ?? new FlowObjectRepository();
		this.deps = {
			commands: deps.commands ?? new FlowCommandService(),
			reads,
			sleep: deps.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms))),
			newKey: deps.newKey ?? newIdempotencyKey
		};
		// Polling only: the workspace submit path of this controller is never used, and nothing is
		// written to the workspace's recent-export list (`store: null`).
		this.poller = new PackageExportController(workspaceId, () => this.emit(), {
			commands: {
				startWorkspaceExport: async () => ({ code: 400, message: '', data: null })
			},
			reads,
			sleep: this.deps.sleep,
			newKey: this.deps.newKey,
			store: null
		});
	}

	get snapshot(): ObjectExportSnapshot {
		const job = this.jobId
			? (this.poller.snapshot.find((view) => view.jobId === this.jobId) ?? null)
			: null;
		return { job, format: this.format };
	}

	/** The key the next submit of `options` sends. Same options, same key. */
	keyFor(options: ObjectExportOptions): string {
		const request = buildObjectExportRequest(options, '');
		const signature = JSON.stringify([request.format, request.include_history]);
		if (!this.intent || this.intent.signature !== signature) {
			this.intent = { key: this.deps.newKey(), signature };
		}
		return this.intent.key;
	}

	async submit(options: ObjectExportOptions): Promise<ObjectExportSubmitOutcome> {
		if (this.submitting) return { status: 'refused' };
		this.submitting = true;
		try {
			const key = this.keyFor(options);
			const result = await this.deps.commands.startObjectExport(
				this.objectId,
				buildObjectExportRequest(options, key)
			);
			if (result.code !== 0 || !result.data) {
				const failure = classifyObjectExportFailure(result);
				if (!failure.retryable) this.intent = null;
				return { status: 'failed', failure };
			}
			this.intent = null;
			this.jobId = result.data.job_id;
			this.format = options.format;
			this.emit();
			void this.poller.poll(result.data.job_id);
			return { status: 'started', jobId: result.data.job_id };
		} finally {
			this.submitting = false;
		}
	}

	/** Downloads a completed job's artifact after the local SHA-256 check. */
	async download(job: FlowExportJob, title: string): Promise<ExportDownloadOutcome> {
		const format = (this.format ?? job.format) as FlowObjectExportFormat;
		return downloadExport(job, objectExportFileName(title, job.job_id, format), this.deps.reads);
	}

	dispose(): void {
		this.poller.dispose();
	}

	private emit(): void {
		this.onChange(this.snapshot);
	}
}
