// What the milestone screens show, and to whom. The server enforces the same rules (owners and
// admins write milestones; an item can only be put in an open milestone of its workspace); these
// helpers keep the screens from offering what the server would refuse.

import type {
	ListMilestonesParams,
	Milestone,
	MilestoneProgress,
	MilestoneStatus
} from '#lib/api/milestones.js';
import type { ApiResult, PaginatedData } from '#lib/api/client.js';
import { isWorkspaceAdminRole, type WorkspaceRole } from '#lib/workspace/home.js';

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (key: string, options?: { values?: Record<string, unknown> }) => string;

/** Longest milestone name the server accepts. */
export const MILESTONE_NAME_MAX = 100;

/** Creating, editing, closing and deleting milestones is for workspace owners and admins. */
export function canManageMilestones(role: WorkspaceRole | null | undefined): boolean {
	return isWorkspaceAdminRole(role);
}

export interface MilestoneProgressView {
	done: number;
	total: number;
	percent: number;
}

/** Progress as the server counted it (`done` follows the finished-state rule of each project). */
export function milestoneProgressOf(milestone: {
	progress?: Pick<MilestoneProgress, 'total' | 'done'> | null;
}): MilestoneProgressView {
	const total = Math.max(0, milestone.progress?.total ?? 0);
	const done = Math.min(total, Math.max(0, milestone.progress?.done ?? 0));
	return { done, total, percent: total > 0 ? Math.round((done / total) * 100) : 0 };
}

export type NameProblem = 'empty' | 'tooLong';

/** The reason a milestone name would be refused, or null. Length counts characters, like the server. */
export function nameProblem(name: string): NameProblem | null {
	const trimmed = name.trim();
	if (!trimmed) return 'empty';
	if ([...trimmed].length > MILESTONE_NAME_MAX) return 'tooLong';
	return null;
}

const DATE_ONLY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** `YYYY-MM-DD` as a local date for display; null for anything else. */
export function parseDateOnly(value: string | null | undefined): Date | null {
	const match = value ? DATE_ONLY.exec(value) : null;
	if (!match) return null;
	const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
	const date = new Date(year, month - 1, day);
	return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day
		? date
		: null;
}

/** The target date in the viewer's language, without a time-zone shift; '' without a date. */
export function formatTargetDate(value: string | null | undefined, lang: string | null): string {
	const date = parseDateOnly(value);
	if (!date) return '';
	return date.toLocaleDateString(lang === 'en' ? 'en-US' : 'zh-CN', {
		year: 'numeric',
		month: '2-digit',
		day: '2-digit'
	});
}

function dayKey(date: Date): string {
	const month = String(date.getMonth() + 1).padStart(2, '0');
	const day = String(date.getDate()).padStart(2, '0');
	return `${date.getFullYear()}-${month}-${day}`;
}

/** An open milestone whose target day has passed (today is not overdue). */
export function isOverdue(
	milestone: Pick<Milestone, 'status' | 'target_date'>,
	now: Date = new Date()
): boolean {
	return (
		milestone.status === 'open' &&
		parseDateOnly(milestone.target_date) !== null &&
		(milestone.target_date as string) < dayKey(now)
	);
}

/** Open milestones first (soonest target first, undated last), then by name. */
export function sortMilestones<T extends Pick<Milestone, 'name' | 'target_date' | 'id'>>(
	milestones: readonly T[],
	locale?: string
): T[] {
	const collator = new Intl.Collator(locale);
	return [...milestones].sort((a, b) => {
		if (a.target_date !== b.target_date) {
			if (!a.target_date) return 1;
			if (!b.target_date) return -1;
			return a.target_date < b.target_date ? -1 : 1;
		}
		return collator.compare(a.name, b.name) || a.id.localeCompare(b.id);
	});
}

export interface MilestoneChoice {
	/** The milestone id; `''` is "No milestone". */
	value: string;
	label: string;
	disabled: boolean;
}

/**
 * The options of a Milestone select: "No milestone", then the open milestones of the workspace, and
 * the item's current milestone when it is closed (shown with its status and disabled: the server
 * refuses a closed milestone as a target, and moving the item out of it is still allowed).
 */
export function milestoneChoices(
	milestones: readonly Pick<Milestone, 'id' | 'name' | 'status'>[],
	currentId: string | null | undefined,
	text: { none: string; closed: string }
): MilestoneChoice[] {
	const collator = new Intl.Collator();
	const listed = milestones
		.filter((milestone) => milestone.status === 'open' || milestone.id === currentId)
		.sort((a, b) => collator.compare(a.name, b.name) || a.id.localeCompare(b.id))
		.map((milestone) => ({
			value: milestone.id,
			label: milestone.status === 'closed' ? `${milestone.name} (${text.closed})` : milestone.name,
			disabled: milestone.status === 'closed'
		}));
	return [{ value: '', label: text.none, disabled: false }, ...listed];
}

/** The name of a milestone in a loaded list, or null when the id is not in it. */
export function milestoneNameOf(
	milestones: readonly Pick<Milestone, 'id' | 'name'>[],
	id: string | null | undefined
): string | null {
	return id ? (milestones.find((milestone) => milestone.id === id)?.name ?? null) : null;
}

/** What a refused milestone call answers; the fields of `ApiResult` that matter here. */
export interface MilestoneFailure {
	message: string;
	messageKey?: string;
	messageValues?: Record<string, string | number>;
	error_code?: string;
}

/**
 * The words for a refused call: a translated sentence for the typed rule the server names, the
 * client's own transport text, and otherwise the generic failure text followed by the server's
 * message.
 */
export function milestoneFailureMessage(translate: Translate, failure: MilestoneFailure): string {
	if (failure.error_code === 'milestone_name_taken') return translate('milestones.error.nameTaken');
	if (failure.messageKey) return translate(failure.messageKey, { values: failure.messageValues });
	return failure.message
		? `${translate('milestones.error.failed')} ${failure.message}`
		: translate('milestones.error.failed');
}

export type MilestoneLister = (
	workspaceId: string,
	params?: ListMilestonesParams
) => Promise<ApiResult<PaginatedData<Milestone>>>;

/** Pages of 100 are read until the last one; at most 20 pages (2,000 milestones). */
const LIST_PAGE_SIZE = 100;
const LIST_PAGE_LIMIT = 20;

/**
 * Every milestone of the workspace with the given status (both statuses without one): the item
 * page and the item list need names and choices, not a page of them. The first failing page ends
 * the read with its failure.
 */
export async function loadAllMilestones(
	list: MilestoneLister,
	workspaceId: string,
	status?: MilestoneStatus
): Promise<ApiResult<Milestone[]>> {
	const statuses: MilestoneStatus[] = status ? [status] : ['open', 'closed'];
	const all: Milestone[] = [];
	for (const one of statuses) {
		for (let page = 1; page <= LIST_PAGE_LIMIT; page += 1) {
			const response = await list(workspaceId, { status: one, page, per_page: LIST_PAGE_SIZE });
			if (response.code !== 0 || !response.data) return { ...response, data: null };
			all.push(...response.data.items);
			if (page >= response.data.total_pages) break;
		}
	}
	return { code: 0, message: '', data: all };
}

/**
 * The line of a `milestone_changed` activity (`detail {from, to}`: milestone ids or null). The name
 * is the one in a loaded list; a milestone that is not in it (deleted, or not loaded) is described
 * without a name, never by its id.
 */
export function milestoneActivityText(
	translate: Translate,
	detail: Record<string, unknown> | null | undefined,
	nameOf: (id: string) => string | null
): string {
	const to = typeof detail?.to === 'string' && detail.to ? detail.to : '';
	if (!to) return translate('milestones.activity.removed');
	const name = nameOf(to);
	return name
		? translate('milestones.activity.set', { values: { name } })
		: translate('milestones.activity.setUnnamed');
}
