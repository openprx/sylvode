// State boundary for the v0.8 package-import wizard. Package bytes are handed directly to the
// HTTP adapter and are never copied into ObjectRepository, IndexedDB, or this session object.

import {
	flowApi,
	type FlowPackageArtifactReceipt,
	type FlowPackageConflictPolicy,
	type FlowPackageExternalReferencePolicy,
	type FlowPackageImportJobReceipt,
	type FlowPackageImportPreview,
	type UploadProgress
} from '$lib/api/flow';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';

/** The machine-readable part of a rejected envelope. `message` is deliberately not carried as a
 * discriminator (`error-mapping-v1.md`: "禁止用英文 message 分支"). */
export interface FlowPackageEnvelope {
	readonly code: number;
	readonly error_code?: string;
	readonly details?: unknown;
}

/** A non-success REST envelope from one of the package endpoints. */
export class FlowPackageRequestError extends Error {
	readonly envelope: FlowPackageEnvelope;

	constructor(result: { code: number; message: string; error_code?: string; details?: unknown }) {
		super(result.message || 'Package import failed');
		this.name = 'FlowPackageRequestError';
		this.envelope = {
			code: result.code,
			...(result.error_code === undefined ? {} : { error_code: result.error_code }),
			...(result.details === undefined ? {} : { details: result.details })
		};
	}
}

function requireData<T>(result: {
	code: number;
	message: string;
	data: T | null;
	error_code?: string;
	details?: unknown;
}): T {
	if (result.code !== 0 || result.data === null) throw new FlowPackageRequestError(result);
	return result.data;
}

/** Which package call failed; selects the wording for legacy (no `error_code`) rejections. */
export type FlowPackageOperation = 'export' | 'upload' | 'preview' | 'commit' | 'report';

export interface FlowPackageFailure {
	readonly messageKey: string;
	readonly values: Readonly<Record<string, string>>;
	/** `checksum_mismatch` / `unsupported_format` / `policy_rejected`: stays on screen and keeps
	 * the wizard out of confirm until the package is uploaded and previewed again. */
	readonly blocking: boolean;
	readonly retryable: boolean;
	readonly forbidden: boolean;
}

const BLOCKING_CODES: ReadonlySet<string> = new Set([
	'checksum_mismatch',
	'unsupported_format',
	'policy_rejected'
]);

/** Every key `classifyPackageFailure` can return besides `flow.error.<stable code>`. */
export const FLOW_PACKAGE_ERROR_KEYS = [
	'flow.package.error.rejected',
	'flow.package.error.unavailable',
	'flow.package.error.expired',
	'flow.export.error.empty',
	'flow.export.error.conflict',
	'flow.import.error.previewRejected',
	'flow.import.error.conflict',
	'flow.import.error.commitConflict',
	'flow.import.error.pending'
] as const;

function legacyFailure(
	messageKey: string,
	options: { retryable?: boolean; forbidden?: boolean } = {}
): FlowPackageFailure {
	return {
		messageKey,
		values: {},
		blocking: false,
		retryable: options.retryable ?? false,
		forbidden: options.forbidden ?? false
	};
}

/**
 * Classifies a rejected package envelope. A typed `error_code` wins; the legacy string-typed
 * envelope (`apps/api/src/error.rs::legacy_response`, no `error_code`) is classified by its
 * numeric business code, worded per operation because the same class means different things
 * (an export `NotFound` is an empty scope; a preview `NotFound` is an expired artifact). The
 * envelope's `message` is never read.
 */
export function classifyPackageFailure(
	envelope: FlowPackageEnvelope,
	operation: FlowPackageOperation
): FlowPackageFailure {
	const flowError = flowErrorFromEnvelope(envelope);
	if (flowError) {
		const values: Record<string, string> = {};
		if (flowError.code === 'limit_exceeded' && typeof envelope.details === 'object') {
			const details = (envelope.details ?? {}) as Record<string, unknown>;
			values.limit = String(details.limit ?? '');
			values.observed = String(details.observed ?? '');
		}
		return {
			messageKey: flowErrorI18nKey(flowError),
			values,
			blocking: BLOCKING_CODES.has(flowError.code),
			retryable: flowError.recoverable,
			forbidden: flowError.code === 'forbidden'
		};
	}
	if (typeof envelope.error_code === 'string') {
		// A typed code this build does not know: fail closed, no retry advertised.
		return legacyFailure('flow.package.error.rejected');
	}
	if (envelope.code >= 500)
		return legacyFailure('flow.package.error.unavailable', { retryable: true });
	switch (envelope.code) {
		case 401:
			return legacyFailure('flow.error.unauthenticated');
		case 403:
			return legacyFailure('flow.error.forbidden', { forbidden: true });
		case 404:
			return legacyFailure(
				operation === 'export' ? 'flow.export.error.empty' : 'flow.package.error.expired'
			);
		case 409:
			if (operation === 'export') return legacyFailure('flow.export.error.conflict');
			if (operation === 'preview') return legacyFailure('flow.import.error.conflict');
			if (operation === 'report')
				return legacyFailure('flow.import.error.pending', { retryable: true });
			return legacyFailure('flow.import.error.commitConflict');
		case 400:
			if (operation === 'preview') return legacyFailure('flow.import.error.previewRejected');
			return legacyFailure('flow.package.error.rejected');
		default:
			return legacyFailure('flow.package.error.rejected');
	}
}

/** Classifies anything a package call threw. A non-envelope throw (a programming error or a
 * refused local precondition) is a permanent rejection. */
export function classifyPackageThrow(
	error: unknown,
	operation: FlowPackageOperation
): FlowPackageFailure {
	if (error instanceof FlowPackageRequestError)
		return classifyPackageFailure(error.envelope, operation);
	return legacyFailure('flow.package.error.rejected');
}

export class FlowPackageImportSession {
	private artifact: FlowPackageArtifactReceipt | null = null;
	private previewReceipt: FlowPackageImportPreview | null = null;
	private previewConflictPolicy: FlowPackageConflictPolicy | null = null;

	constructor(private readonly workspaceId: string) {}

	async upload(
		file: Blob,
		filename: string,
		idempotencyKey: string,
		signal?: AbortSignal,
		onProgress?: (progress: UploadProgress) => void
	): Promise<FlowPackageArtifactReceipt> {
		this.reset();
		const result = await flowApi.uploadPackageArtifact(
			this.workspaceId,
			file,
			filename,
			idempotencyKey,
			signal,
			onProgress
		);
		// A cancelled upload never leaves an artifact behind, even if the response raced the abort.
		if (signal?.aborted)
			throw new FlowPackageRequestError({ code: 499, message: 'Upload cancelled' });
		this.artifact = requireData(result);
		return this.artifact;
	}

	/** Forgets the artifact and any preview (cancel, new file). */
	reset(): void {
		this.artifact = null;
		this.previewReceipt = null;
		this.previewConflictPolicy = null;
	}

	get hasArtifact(): boolean {
		return this.artifact !== null;
	}

	get hasPreview(): boolean {
		return this.previewReceipt !== null;
	}

	async preview(input: {
		projectMapping?: Record<string, string | null>;
		externalReferencePolicy: FlowPackageExternalReferencePolicy;
		conflictPolicy: FlowPackageConflictPolicy;
		includeHistory: boolean;
		idempotencyKey: string;
	}): Promise<FlowPackageImportPreview> {
		if (!this.artifact) throw new Error('A verified package artifact is required before preview');
		this.previewReceipt = null;
		this.previewConflictPolicy = null;
		this.previewReceipt = requireData(
			await flowApi.previewPackageImport(this.workspaceId, {
				artifact_id: this.artifact.artifact_id,
				project_mapping: input.projectMapping ?? {},
				external_reference_policy: input.externalReferencePolicy,
				conflict_policy: input.conflictPolicy,
				include_history: input.includeHistory,
				idempotency_key: input.idempotencyKey
			})
		);
		this.previewConflictPolicy = input.conflictPolicy;
		return this.previewReceipt;
	}

	async commit(input: {
		exactPackageSha256: string;
		idempotencyKey: string;
	}): Promise<FlowPackageImportJobReceipt> {
		const preview = this.previewReceipt;
		const conflictPolicy = this.previewConflictPolicy;
		if (!preview || !conflictPolicy)
			throw new Error('A successful server preview is required before commit');
		if (input.exactPackageSha256 !== preview.package_sha256) {
			throw new Error('The confirmed package hash does not match the server preview');
		}
		return requireData(
			await flowApi.commitPackageImport(this.workspaceId, preview.preview_id, {
				package_sha256: preview.package_sha256,
				mapping_hash: preview.mapping_hash,
				conflict_policy: conflictPolicy,
				confirm: true,
				idempotency_key: input.idempotencyKey
			})
		);
	}
}
