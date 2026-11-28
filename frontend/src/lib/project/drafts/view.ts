/** Pure helpers of the drafts list and the review dialog. */
import type { DraftStatus, DraftSummary, ValidationIssue } from './wire';

export type Tone = 'neutral' | 'success' | 'warning' | 'danger';

export const STATUS_FILTERS: readonly DraftStatus[] = ['pending', 'approved', 'rejected'];

export function statusTone(status: DraftStatus): Tone {
	switch (status) {
		case 'approved':
			return 'success';
		case 'pending':
			return 'warning';
		case 'failed':
			return 'danger';
		default:
			return 'neutral';
	}
}

export function riskTone(risk: string): Tone {
	switch (risk) {
		case 'read':
			return 'neutral';
		case 'write':
			return 'warning';
		default:
			return 'danger';
	}
}

export type Words = (key: string, values?: Record<string, string>) => string;

/**
 * One line that says what a draft is about: a form's key or id, the slot and the scope, or the
 * page of a site. A site page draft without a page is a title change (update) or a new page.
 */
export function targetLine(draft: Pick<DraftSummary, 'kind' | 'target'>, words?: Words): string {
	const target = draft.target;
	const text = (key: string): string => {
		const value = target[key];
		return typeof value === 'string' ? value : '';
	};
	if (draft.kind === 'form_schema') {
		return text('form_id') ? `form ${text('form_id')}` : `new form ${text('key')}`;
	}
	if (draft.kind === 'layout') return `${text('slot_id')} / ${text('scope_kind')}`;
	if (draft.kind === 'package_upload') return `${text('package_key')}@${text('version')}`;
	if (draft.kind === 'site_page' && words) {
		const op = text('op');
		const base = draft.target.base_page;
		const route =
			typeof base === 'object' &&
			base !== null &&
			typeof (base as { route?: unknown }).route === 'string'
				? (base as { route: string }).route
				: '';
		if (text('page_id') === '' && op === 'update') return words('drafts.target.siteTitle');
		if (op === 'create') return words('drafts.target.pageCreate');
		return words(`drafts.target.page_${op}`, { route });
	}
	return draft.kind;
}

/** What a reviewer checks first in a package upload draft: the package and who signed it. */
export interface UploadFacts {
	packageKey: string;
	version: string;
	kind: string;
	sizeBytes: number;
	fileCount: number;
	publisher: string;
	signerKeyId: string;
	customCode: boolean;
}

/** Reads the `spec` of a package upload draft; null when it is not shaped like one. */
export function uploadFacts(spec: unknown): UploadFacts | null {
	if (typeof spec !== 'object' || spec === null) return null;
	const value = spec as Record<string, unknown>;
	const publisher = value.publisher as { id?: unknown; key_id?: unknown } | null | undefined;
	if (typeof value.package_key !== 'string' || typeof value.version !== 'string') return null;
	return {
		packageKey: value.package_key,
		version: value.version,
		kind: typeof value.kind === 'string' ? value.kind : '',
		sizeBytes: typeof value.size_bytes === 'number' ? value.size_bytes : 0,
		fileCount: typeof value.file_count === 'number' ? value.file_count : 0,
		publisher: typeof publisher?.id === 'string' ? publisher.id : '',
		signerKeyId: typeof publisher?.key_id === 'string' ? publisher.key_id : '',
		customCode: value.custom_code === true
	};
}

/** A byte count as KiB or MiB for the review line. */
export function formatBytes(bytes: number): string {
	if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
	if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
	return `${bytes} B`;
}

/** Whether the viewer can approve: a person who administers the workspace, on a valid pending draft. */
export function canApprove(
	draft: Pick<DraftSummary, 'status' | 'valid'>,
	isAdmin: boolean
): boolean {
	return isAdmin && draft.status === 'pending' && draft.valid;
}

export function issueLine(issue: ValidationIssue): string {
	const where = issue.path && issue.path !== '' ? issue.path : '/';
	return issue.message ? `${where}  ${issue.code}: ${issue.message}` : `${where}  ${issue.code}`;
}

/** The new digest a `draft_stale` refusal carries, so the dialog can reload the state it names. */
export function staleDigest(details: unknown): string | null {
	if (typeof details !== 'object' || details === null) return null;
	const digest = (details as { review_digest?: unknown }).review_digest;
	return typeof digest === 'string' ? digest : null;
}
