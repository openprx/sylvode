// Five-step package import wizard (`contracts/ui-surface-v1.md` "Package round-trip UI (v0.8)"):
// choose a local `.sylvode-flow.zip` -> streaming upload (progress, cancel) -> mapping and
// policies -> server preview -> exact-hash confirmation and commit. It composes
// `FlowPackageImportSession`, which owns the HTTP calls and guarantees that commit sends the
// preview's own `package_sha256` / `mapping_hash` / `conflict_policy`.
//
// Rules this module owns, so they are unit-testable without a component:
// - steps cannot be skipped: `advance` only moves one step forward and only when
//   `canAdvance(step)` holds; `goTo` only moves backwards; going back keeps every entered value;
// - `checksum_mismatch` / `unsupported_format` / `policy_rejected` stay on screen and keep the
//   wizard out of confirm until a new upload or a successful re-preview;
// - commit needs the user's explicit hash acknowledgement, an unexpired preview and no blocking
//   error; the hash it sends is the server preview's, never one computed or typed locally;
// - one idempotency key per intent: the upload key lives as long as the chosen file, a preview
//   key as long as the exact option set (and is dropped once that preview succeeded, so an
//   explicit re-preview after expiry is a new request), the commit key as long as the preview;
// - the package bytes are handed to the upload and then dropped -- nothing is parsed locally and
//   nothing is written to ObjectRepository or IndexedDB;
// - a successful preview can still refuse confirm (`rest-api-v1.md` v0.8 `conflict_details`,
//   2026-10-10): `project_unmapped` sends the wizard back to the mapping step with one row per
//   unmapped source project (its title), each needing an explicit target or "unprojected" and a
//   new preview; `lineage_exists` under `reject_existing` lists the already-imported objects and
//   keeps confirm closed until the user previews again with `reuse_import_lineage`.

import type {
	FlowPackageArtifactReceipt,
	FlowPackageConflictDetail,
	FlowPackageConflictPolicy,
	FlowPackageExternalReferencePolicy,
	FlowPackageImportPreview,
	UploadProgress
} from '#lib/api/flow.js';
import { DEFAULT_FLOW_LIMITS } from './limits';
import { newIdempotencyKey } from './package-export';
import {
	FlowPackageImportSession,
	classifyPackageThrow,
	type FlowPackageFailure
} from './package-import';

export type { FlowPackageConflictDetail, FlowPackageImportPreview, FlowPackageFailure };

export type ProjectUnmappedConflict = Extract<
	FlowPackageConflictDetail,
	{ kind: 'project_unmapped' }
>;
export type LineageExistsConflict = Extract<FlowPackageConflictDetail, { kind: 'lineage_exists' }>;

/** The preview's structured conflicts split by kind. A preview without `conflict_details` (an
 * older server) has none; unknown kinds are ignored rather than guessed at. */
export function previewConflicts(
	preview: Pick<FlowPackageImportPreview, 'conflict_details'> | null
): {
	readonly unmapped: readonly ProjectUnmappedConflict[];
	readonly lineage: readonly LineageExistsConflict[];
} {
	const unmapped: ProjectUnmappedConflict[] = [];
	const lineage: LineageExistsConflict[] = [];
	for (const detail of preview?.conflict_details ?? []) {
		if (detail.kind === 'project_unmapped') unmapped.push(detail);
		else if (detail.kind === 'lineage_exists') lineage.push(detail);
	}
	return { unmapped, lineage };
}

/** Why a successful preview still cannot be confirmed, or `null` when it can. */
export type PreviewHold = 'project_unmapped' | 'lineage_exists';

export const PREVIEW_HOLD_KEYS: Readonly<Record<PreviewHold, string>> = {
	project_unmapped: 'flow.import.preview.hold.projectUnmapped',
	lineage_exists: 'flow.import.preview.hold.lineageExists'
};

export const PACKAGE_FILE_EXTENSION = '.sylvode-flow.zip';

/** `limits-v1.md#import_archive_bytes_max`, already carried by `DEFAULT_FLOW_LIMITS`. */
export const IMPORT_ARCHIVE_BYTES_MAX = DEFAULT_FLOW_LIMITS.importArchiveBytesMax;

export const WIZARD_STEPS = ['file', 'upload', 'options', 'preview', 'confirm'] as const;
export type WizardStep = (typeof WIZARD_STEPS)[number];

export type FileRejection = 'extension' | 'empty' | 'too_large';

export type UploadState = 'idle' | 'uploading' | 'uploaded' | 'failed' | 'cancelled';
export type PreviewState = 'idle' | 'running' | 'ready' | 'failed';

// i18n keys, spelled out statically so the unit tests can check every one exists in zh and en.
export const STEP_KEYS: Readonly<Record<WizardStep, string>> = {
	file: 'flow.import.step.file',
	upload: 'flow.import.step.upload',
	options: 'flow.import.step.options',
	preview: 'flow.import.step.preview',
	confirm: 'flow.import.step.confirm'
};

export const FILE_REJECTION_KEYS: Readonly<Record<FileRejection, string>> = {
	extension: 'flow.import.file.rejectExtension',
	empty: 'flow.import.file.rejectEmpty',
	too_large: 'flow.import.file.rejectTooLarge'
};

export const UPLOAD_STATE_KEYS: Readonly<Record<UploadState, string>> = {
	idle: 'flow.import.upload.state.idle',
	uploading: 'flow.import.upload.state.uploading',
	uploaded: 'flow.import.upload.state.uploaded',
	failed: 'flow.import.upload.state.failed',
	cancelled: 'flow.import.upload.state.cancelled'
};

export const EXTERNAL_POLICY_KEYS: Readonly<Record<FlowPackageExternalReferencePolicy, string>> = {
	reject: 'flow.import.options.external.reject',
	detach: 'flow.import.options.external.detach'
};

export const CONFLICT_POLICY_KEYS: Readonly<Record<FlowPackageConflictPolicy, string>> = {
	reject_existing: 'flow.import.options.conflict.reject_existing',
	reuse_import_lineage: 'flow.import.options.conflict.reuse_import_lineage'
};

/** `estimated_changes` kinds the server emits (`package_import.rs::estimated_changes`). */
export const ESTIMATE_KIND_KEYS: Readonly<Record<string, string>> = {
	objects: 'flow.import.preview.kind.objects',
	documents: 'flow.import.preview.kind.documents',
	relations: 'flow.import.preview.kind.relations'
};

export interface ProjectMappingRow {
	readonly sourceProjectId: string;
	/** `null` maps the source project to "unprojected". */
	readonly targetProjectId: string | null;
	/** The source project's title from the preview (`conflict_details[].title`), when known. */
	readonly title?: string | null;
	/** A row the wizard added for a `project_unmapped` conflict: the user has not chosen a target
	 * yet, so the mapping is not valid (the server must never guess the target). */
	readonly unresolved?: boolean;
}

export interface ImportOptions {
	readonly externalReferencePolicy: FlowPackageExternalReferencePolicy;
	readonly conflictPolicy: FlowPackageConflictPolicy;
	readonly includeHistory: boolean;
	readonly projectMapping: readonly ProjectMappingRow[];
}

export const DEFAULT_IMPORT_OPTIONS: ImportOptions = {
	externalReferencePolicy: 'reject',
	conflictPolicy: 'reject_existing',
	includeHistory: false,
	projectMapping: []
};

export interface WizardSnapshot {
	readonly step: WizardStep;
	readonly fileName: string | null;
	readonly fileSize: number | null;
	readonly fileRejection: FileRejection | null;
	readonly uploadState: UploadState;
	readonly progress: UploadProgress | null;
	readonly artifact: FlowPackageArtifactReceipt | null;
	readonly options: ImportOptions;
	readonly optionsValid: boolean;
	readonly previewState: PreviewState;
	readonly preview: FlowPackageImportPreview | null;
	readonly failure: FlowPackageFailure | null;
	readonly blockingFailure: FlowPackageFailure | null;
	readonly hashAcknowledged: boolean;
	readonly committing: boolean;
	readonly expired: boolean;
	/** Source projects the last preview reported as unmapped; shown on the mapping step. */
	readonly unmappedProjects: readonly ProjectUnmappedConflict[];
	/** Why the current preview cannot be confirmed, or `null`. */
	readonly previewHold: PreviewHold | null;
	/** `canAdvance(step)` for the current step, captured with the rest of the snapshot so a
	 * component reading only the snapshot re-renders when it changes. */
	readonly canAdvance: boolean;
	readonly canCommit: boolean;
}

export type CommitOutcome =
	| { readonly status: 'committed'; readonly importId: string }
	| { readonly status: 'refused' }
	| { readonly status: 'failed'; readonly failure: FlowPackageFailure };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function validatePackageFile(name: string, size: number): FileRejection | null {
	if (!name.toLowerCase().endsWith(PACKAGE_FILE_EXTENSION)) return 'extension';
	if (!(size > 0)) return 'empty';
	if (size > IMPORT_ARCHIVE_BYTES_MAX) return 'too_large';
	return null;
}

export function mappingRowsValid(rows: readonly ProjectMappingRow[]): boolean {
	const seen = new Set<string>();
	for (const row of rows) {
		if (row.unresolved) return false;
		const source = row.sourceProjectId.trim().toLowerCase();
		if (!UUID.test(source) || seen.has(source)) return false;
		if (row.targetProjectId !== null && !UUID.test(row.targetProjectId)) return false;
		seen.add(source);
	}
	return true;
}

export function buildProjectMapping(
	rows: readonly ProjectMappingRow[]
): Record<string, string | null> {
	const mapping: Record<string, string | null> = {};
	for (const row of rows) mapping[row.sourceProjectId.trim().toLowerCase()] = row.targetProjectId;
	return mapping;
}

function optionsSignature(artifactId: string, options: ImportOptions): string {
	return JSON.stringify([
		artifactId,
		options.externalReferencePolicy,
		options.conflictPolicy,
		options.includeHistory,
		Object.entries(buildProjectMapping(options.projectMapping)).sort(([a], [b]) =>
			a.localeCompare(b)
		)
	]);
}

export interface PackageWizardDeps {
	readonly session: Pick<FlowPackageImportSession, 'upload' | 'preview' | 'commit' | 'reset'>;
	readonly newKey: () => string;
	readonly now: () => number;
}

export class PackageImportWizard {
	private readonly deps: PackageWizardDeps;
	private currentStep: WizardStep = 'file';
	private file: Blob | null = null;
	private fileName: string | null = null;
	private fileSize: number | null = null;
	private fileRejection: FileRejection | null = null;
	private uploadKey: string | null = null;
	private uploadState: UploadState = 'idle';
	private progress: UploadProgress | null = null;
	private abort: AbortController | null = null;
	private artifact: FlowPackageArtifactReceipt | null = null;
	private options: ImportOptions = DEFAULT_IMPORT_OPTIONS;
	private previewIntent: { key: string; signature: string } | null = null;
	private previewSignature: string | null = null;
	private previewState: PreviewState = 'idle';
	private preview: FlowPackageImportPreview | null = null;
	private failure: FlowPackageFailure | null = null;
	private blockingFailure: FlowPackageFailure | null = null;
	private hashAcknowledged = false;
	private commitIntent: { key: string; previewId: string } | null = null;
	private committing = false;
	private unmappedProjects: readonly ProjectUnmappedConflict[] = [];

	constructor(
		workspaceId: string,
		private readonly onChange: (snapshot: WizardSnapshot) => void = () => {},
		deps: Partial<PackageWizardDeps> = {}
	) {
		this.deps = {
			session: deps.session ?? new FlowPackageImportSession(workspaceId),
			newKey: deps.newKey ?? newIdempotencyKey,
			now: deps.now ?? Date.now
		};
	}

	get step(): WizardStep {
		return this.currentStep;
	}

	snapshot(): WizardSnapshot {
		return {
			step: this.currentStep,
			fileName: this.fileName,
			fileSize: this.fileSize,
			fileRejection: this.fileRejection,
			uploadState: this.uploadState,
			progress: this.progress,
			artifact: this.artifact,
			options: this.options,
			optionsValid: mappingRowsValid(this.options.projectMapping),
			previewState: this.previewState,
			preview: this.preview,
			failure: this.failure,
			blockingFailure: this.blockingFailure,
			hashAcknowledged: this.hashAcknowledged,
			committing: this.committing,
			expired: this.isExpired(),
			unmappedProjects: this.unmappedProjects,
			previewHold: this.previewHold(),
			canAdvance: this.canAdvance(),
			canCommit: this.canCommit()
		};
	}

	/** Whether `step` is complete, i.e. the wizard may move past it. */
	canAdvance(step: WizardStep = this.currentStep): boolean {
		switch (step) {
			case 'file':
				// After a successful upload the bytes are dropped; the uploaded artifact stands in.
				return (this.file !== null || this.artifact !== null) && this.fileRejection === null;
			case 'upload':
				return this.artifact !== null && this.uploadState === 'uploaded';
			case 'options':
				return this.artifact !== null && mappingRowsValid(this.options.projectMapping);
			case 'preview':
				return (
					this.preview !== null &&
					this.blockingFailure === null &&
					!this.isExpired() &&
					this.previewHold() === null
				);
			case 'confirm':
				return false;
		}
	}

	/** Whether every step before `step` is complete (so `step` is reachable at all). */
	reachable(step: WizardStep): boolean {
		const index = WIZARD_STEPS.indexOf(step);
		return WIZARD_STEPS.slice(0, index).every((earlier) => this.canAdvance(earlier));
	}

	/**
	 * Moves exactly one step forward when the current step is complete. Leaving `options` runs the
	 * server preview first and always lands on `preview`, where its result or error is shown.
	 */
	async advance(): Promise<boolean> {
		const index = WIZARD_STEPS.indexOf(this.currentStep);
		if (index >= WIZARD_STEPS.length - 1 || !this.canAdvance()) return false;
		if (this.currentStep === 'options') {
			await this.runPreview();
			// An unmapped source project keeps the wizard on the mapping step.
			if (this.currentStep === 'options' && this.unmappedProjects.length > 0) {
				this.emit();
				return false;
			}
		}
		this.currentStep = WIZARD_STEPS[index + 1];
		this.emit();
		return true;
	}

	/** Backwards navigation only; every value entered so far is kept. */
	goTo(step: WizardStep): boolean {
		if (WIZARD_STEPS.indexOf(step) > WIZARD_STEPS.indexOf(this.currentStep)) return false;
		if (this.uploadState === 'uploading' || this.committing) return false;
		this.currentStep = step;
		this.emit();
		return true;
	}

	back(): boolean {
		const index = WIZARD_STEPS.indexOf(this.currentStep);
		return index > 0 ? this.goTo(WIZARD_STEPS[index - 1]) : false;
	}

	/** Chooses a new local file. Everything downstream of the file is discarded. */
	selectFile(file: Blob | null, name: string | null): FileRejection | null {
		if (this.uploadState === 'uploading' || this.committing) return null;
		this.resetFromUpload();
		if (!file || name === null) {
			this.file = null;
			this.fileName = null;
			this.fileSize = null;
			this.fileRejection = null;
			this.emit();
			return null;
		}
		this.fileName = name;
		this.fileSize = file.size;
		this.fileRejection = validatePackageFile(name, file.size);
		this.file = this.fileRejection === null ? file : null;
		this.uploadKey = this.fileRejection === null ? this.deps.newKey() : null;
		this.emit();
		return this.fileRejection;
	}

	async startUpload(): Promise<boolean> {
		if (this.currentStep !== 'upload' || this.uploadState === 'uploading') return false;
		const file = this.file;
		const name = this.fileName;
		if (!file || !name || !this.uploadKey) return false;
		const controller = new AbortController();
		this.abort = controller;
		this.uploadState = 'uploading';
		this.progress = { loaded: 0, total: file.size };
		this.failure = null;
		this.emit();
		try {
			const artifact = await this.deps.session.upload(
				file,
				name,
				this.uploadKey,
				controller.signal,
				(progress) => {
					if (this.abort !== controller) return;
					this.progress = progress;
					this.emit();
				}
			);
			if (this.abort !== controller) return false;
			this.abort = null;
			this.artifact = artifact;
			this.uploadState = 'uploaded';
			this.progress = { loaded: artifact.size, total: artifact.size };
			this.blockingFailure = null;
			// The bytes are on the server now; the wizard keeps only the name and size.
			this.file = null;
			this.emit();
			return true;
		} catch (error) {
			if (this.abort !== controller) return false;
			this.abort = null;
			if (controller.signal.aborted) {
				this.markCancelled();
				return false;
			}
			const failure = classifyPackageThrow(error, 'upload');
			this.uploadState = 'failed';
			this.failure = failure;
			if (failure.blocking) this.blockingFailure = failure;
			this.emit();
			return false;
		}
	}

	/** Cancels a running upload. The session forgets any artifact; the chosen file is kept so the
	 * user can start the upload again. */
	cancelUpload(): void {
		const controller = this.abort;
		if (!controller) return;
		this.abort = null;
		controller.abort();
		this.markCancelled();
	}

	setOptions(patch: Partial<ImportOptions>): void {
		if (this.committing) return;
		this.options = { ...this.options, ...patch };
		if (
			this.preview &&
			this.artifact &&
			optionsSignature(this.artifact.artifact_id, this.options) !== this.previewSignature
		) {
			// A preview is only valid for the exact options it was computed for.
			this.dropPreview();
		}
		this.emit();
	}

	/** Runs (or re-runs) the server preview for the current options. */
	async runPreview(): Promise<boolean> {
		const artifact = this.artifact;
		if (!artifact || !mappingRowsValid(this.options.projectMapping) || this.committing)
			return false;
		const options = this.options;
		const signature = optionsSignature(artifact.artifact_id, options);
		if (!this.previewIntent || this.previewIntent.signature !== signature) {
			this.previewIntent = { key: this.deps.newKey(), signature };
		}
		const intent = this.previewIntent;
		this.dropPreview();
		this.previewState = 'running';
		this.failure = null;
		this.emit();
		try {
			const preview = await this.deps.session.preview({
				projectMapping: buildProjectMapping(options.projectMapping),
				externalReferencePolicy: options.externalReferencePolicy,
				conflictPolicy: options.conflictPolicy,
				includeHistory: options.includeHistory,
				idempotencyKey: intent.key
			});
			if (this.previewIntent !== intent) return false;
			this.previewIntent = null;
			this.preview = preview;
			this.previewSignature = signature;
			this.previewState = 'ready';
			this.blockingFailure = null;
			const { unmapped } = previewConflicts(preview);
			this.unmappedProjects = unmapped;
			if (unmapped.length > 0) this.returnToMapping(unmapped);
			this.emit();
			return true;
		} catch (error) {
			if (this.previewIntent !== intent) return false;
			const failure = classifyPackageThrow(error, 'preview');
			if (!failure.retryable) this.previewIntent = null;
			this.previewState = 'failed';
			this.failure = failure;
			if (failure.blocking) this.blockingFailure = failure;
			this.emit();
			return false;
		}
	}

	/** Why the current preview cannot be confirmed although it succeeded. */
	previewHold(): PreviewHold | null {
		const preview = this.preview;
		if (!preview) return null;
		const { unmapped, lineage } = previewConflicts(preview);
		if (unmapped.length > 0) return 'project_unmapped';
		if (lineage.length > 0 && this.previewConflictPolicy(preview) === 'reject_existing') {
			return 'lineage_exists';
		}
		return null;
	}

	/**
	 * `lineage_exists` under `reject_existing`: switch the conflict policy to
	 * `reuse_import_lineage` and preview again (a new request with a new key). Refused unless the
	 * current preview is held for exactly that reason.
	 */
	async previewWithReuse(): Promise<boolean> {
		if (this.previewHold() !== 'lineage_exists' || this.committing) return false;
		this.setOptions({ conflictPolicy: 'reuse_import_lineage' });
		return this.runPreview();
	}

	acknowledgeHash(value: boolean): void {
		if (this.committing) return;
		this.hashAcknowledged = value && this.preview !== null;
		this.emit();
	}

	isExpired(): boolean {
		if (!this.preview) return false;
		const expiresAt = Date.parse(this.preview.expires_at);
		return !Number.isFinite(expiresAt) || expiresAt <= this.deps.now();
	}

	/** Whole seconds until the preview expires (0 once expired), or `null` without a preview. */
	secondsUntilExpiry(): number | null {
		if (!this.preview) return null;
		const expiresAt = Date.parse(this.preview.expires_at);
		if (!Number.isFinite(expiresAt)) return 0;
		return Math.max(0, Math.floor((expiresAt - this.deps.now()) / 1000));
	}

	canCommit(): boolean {
		return (
			this.currentStep === 'confirm' &&
			this.preview !== null &&
			this.hashAcknowledged &&
			this.blockingFailure === null &&
			!this.isExpired() &&
			!this.committing
		);
	}

	async commit(): Promise<CommitOutcome> {
		const preview = this.preview;
		if (!preview || !this.canCommit()) return { status: 'refused' };
		if (!this.commitIntent || this.commitIntent.previewId !== preview.preview_id) {
			this.commitIntent = { key: this.deps.newKey(), previewId: preview.preview_id };
		}
		const intent = this.commitIntent;
		this.committing = true;
		this.failure = null;
		this.emit();
		try {
			// The hash is the server preview's own value; `FlowPackageImportSession.commit` sends the
			// preview's hash, mapping hash and conflict policy and refuses any other hash.
			const receipt = await this.deps.session.commit({
				exactPackageSha256: preview.package_sha256,
				idempotencyKey: intent.key
			});
			this.committing = false;
			this.emit();
			return { status: 'committed', importId: receipt.import_id };
		} catch (error) {
			const failure = classifyPackageThrow(error, 'commit');
			this.committing = false;
			if (!failure.retryable) this.commitIntent = null;
			this.failure = failure;
			if (failure.blocking) this.blockingFailure = failure;
			if (failure.cause === 'project_unmapped') {
				// The frozen mapping is incomplete: the preview is useless, the mapping needs work.
				this.dropPreview();
				this.currentStep = 'options';
			}
			this.emit();
			return { status: 'failed', failure };
		}
	}

	/** The conflict policy the preview was computed for (its frozen mapping), falling back to the
	 * options it was requested with. */
	private previewConflictPolicy(preview: FlowPackageImportPreview): FlowPackageConflictPolicy {
		const frozen = (preview.mapping as { conflict_policy?: unknown }).conflict_policy;
		return frozen === 'reject_existing' || frozen === 'reuse_import_lineage'
			? frozen
			: this.options.conflictPolicy;
	}

	/** Adds one unresolved row per unmapped source project (without a target: the user must pick
	 * one or "unprojected") and moves back to the mapping step. The preview is dropped because its
	 * options are no longer the current ones. */
	private returnToMapping(unmapped: readonly ProjectUnmappedConflict[]): void {
		const known = new Set(
			this.options.projectMapping.map((row) => row.sourceProjectId.trim().toLowerCase())
		);
		const added: ProjectMappingRow[] = unmapped
			.filter((conflict) => !known.has(conflict.source_project_id.toLowerCase()))
			.map((conflict) => ({
				sourceProjectId: conflict.source_project_id,
				targetProjectId: null,
				title: conflict.title,
				unresolved: true
			}));
		this.options = {
			...this.options,
			projectMapping: [...this.options.projectMapping, ...added]
		};
		this.dropPreview();
		this.currentStep = 'options';
	}

	private markCancelled(): void {
		this.deps.session.reset();
		this.artifact = null;
		this.uploadState = 'cancelled';
		this.progress = null;
		this.failure = null;
		this.dropPreview();
		this.emit();
	}

	private dropPreview(): void {
		this.preview = null;
		this.previewSignature = null;
		this.previewState = 'idle';
		this.hashAcknowledged = false;
		this.commitIntent = null;
	}

	private resetFromUpload(): void {
		this.abort?.abort();
		this.abort = null;
		this.deps.session.reset();
		this.uploadKey = null;
		this.uploadState = 'idle';
		this.progress = null;
		this.artifact = null;
		this.previewIntent = null;
		this.failure = null;
		this.blockingFailure = null;
		this.unmappedProjects = [];
		// Rows added for another package's unmapped projects do not describe the new file.
		this.options = {
			...this.options,
			projectMapping: this.options.projectMapping.filter((row) => !row.unresolved)
		};
		this.dropPreview();
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}
