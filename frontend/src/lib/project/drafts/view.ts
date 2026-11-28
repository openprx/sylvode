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
