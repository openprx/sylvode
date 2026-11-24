// What the linked-pages screens show. The server decides who may see a title (restricted pages
// arrive with `title: null`) and sends no display text for the states: the words are client
// strings. A title is never invented: without one the state text stands in.

import type { PageLink, PageLinkAccess, PageWorkItems } from '#lib/api/page-links.js';
import type { FlowSearchHit } from '#lib/api/flow.js';

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (key: string, options?: { values?: Record<string, unknown> }) => string;

/** The most pages one work item can link (the server refuses the 51st). */
export const MAX_PAGE_LINKS = 50;

/** The picker asks the server only for this many hits at a time. */
export const PICKER_LIMIT = 20;

/** Whether the item may take one more link; the client says so before the server has to. */
export function canLinkMore(count: number): boolean {
	return count < MAX_PAGE_LINKS;
}

export interface PageLinkView {
	objectId: string;
	/** The page's own title, or null: then `stateKey` is shown. */
	title: string | null;
	/** Locale key of the stand-in text when there is no title. */
	stateKey: string | null;
	restricted: boolean;
	inTrash: boolean;
}

/** The title (when the caller may read it) and the state badges of one link. */
export function pageLinkView(link: PageLink): PageLinkView {
	const title = typeof link.title === 'string' && link.title.trim() ? link.title : null;
	const restricted = link.access === 'restricted';
	return {
		objectId: link.object_id,
		title,
		stateKey: title ? null : restricted ? 'pageLinks.restricted' : 'pageLinks.untitled',
		restricted,
		inTrash: link.lifecycle === 'archived'
	};
}

/** Search hits a person can link: active pages only, and none that are linked already. */
export function pickerCandidates(
	hits: readonly FlowSearchHit[],
	linkedIds: ReadonlySet<string>
): Array<{ id: string; title: string }> {
	const seen = new Set<string>();
	const out: Array<{ id: string; title: string }> = [];
	for (const hit of hits) {
		const object = hit.object;
		if (object.object_type !== 'page' || object.lifecycle_status !== 'active') continue;
		if (linkedIds.has(object.id) || seen.has(object.id)) continue;
		seen.add(object.id);
		out.push({ id: object.id, title: object.title });
	}
	return out;
}

export interface WorkItemsSummary {
	hiddenCount: number;
	truncated: boolean;
	empty: boolean;
}

/** The part of the page side's answer the panel words: items are listed, the rest only counted. */
export function workItemsSummary(data: PageWorkItems): WorkItemsSummary {
	const hiddenCount = Math.max(0, Math.trunc(data.hidden_count || 0));
	return {
		hiddenCount,
		truncated: data.truncated === true,
		empty: data.items.length === 0 && hiddenCount === 0
	};
}

type PageDetail = {
	title: string | null;
	access: PageLinkAccess | 'removed' | null;
};

function pageOf(detail: Record<string, unknown> | null | undefined): PageDetail | null {
	const page = detail?.page;
	if (!page || typeof page !== 'object') return null;
	const raw = page as Record<string, unknown>;
	const title = typeof raw.title === 'string' && raw.title.trim() ? raw.title : null;
	const access =
		raw.access === 'visible' || raw.access === 'restricted' || raw.access === 'removed'
			? raw.access
			: null;
	return { title, access };
}

/**
 * The sentence for a `page_linked` / `page_unlinked` entry: the title when the server gave one,
 * restricted or removed wording otherwise, never an id. `null` for any other action.
 */
export function pageActivityText(
	translate: Translate,
	action: string,
	detail: Record<string, unknown> | null | undefined
): string | null {
	if (action !== 'page_linked' && action !== 'page_unlinked') return null;
	const verb = action === 'page_linked' ? 'linked' : 'unlinked';
	const page = pageOf(detail);
	if (page?.title) {
		return translate(`pageLinks.activity.${verb}`, { values: { title: page.title } });
	}
	if (page?.access === 'restricted') return translate(`pageLinks.activity.${verb}Restricted`);
	if (page?.access === 'removed') return translate(`pageLinks.activity.${verb}Removed`);
	return translate(`pageLinks.activity.${verb}Generic`);
}
