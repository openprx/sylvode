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

/** One line that says what a draft is about: a form's key or id, or the slot and the scope. */
export function targetLine(draft: Pick<DraftSummary, 'kind' | 'target'>): string {
	const target = draft.target;
	const text = (key: string): string => {
		const value = target[key];
		return typeof value === 'string' ? value : '';
	};
	if (draft.kind === 'form_schema') {
		return text('form_id') ? `form ${text('form_id')}` : `new form ${text('key')}`;
	}
	if (draft.kind === 'layout') return `${text('slot_id')} / ${text('scope_kind')}`;
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
