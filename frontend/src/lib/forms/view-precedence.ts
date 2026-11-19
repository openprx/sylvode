/**
 * Which view a form opens with on a cold load (page load, reload, opening a form).
 *
 * Precedence: a `?view=` link the user can follow, then the user's own last explicit choice, then
 * the form's admin default, then the built-in "All records" table. The built-in table always
 * exists, so a first visit never lands on whatever view a template happened to create first.
 */

import type { FormView } from '#lib/api/forms.js';

/** Id of the built-in table. It is not stored in `form_views`. */
export const ALL_RECORDS_VIEW_ID = 'all';

export type ViewChoiceSource = 'url' | 'remembered' | 'form-default' | 'built-in';

export interface ResolveInitialViewInput {
	/** Already filtered to the views the user can see (the API never lists other people's private views). */
	views: Array<Pick<FormView, 'id' | 'config'>>;
	urlViewId?: string | null;
	rememberedViewId?: string | null;
}

export interface ResolvedView {
	/** A saved view id or {@link ALL_RECORDS_VIEW_ID}. */
	viewId: string;
	source: ViewChoiceSource;
	/** True when a remembered id was given but no longer names a view the user can see. */
	dropRemembered: boolean;
}

function viewIsDefault(view: Pick<FormView, 'config'>): boolean {
	return view.config?.is_default === true;
}

function knownViewId(
	views: ResolveInitialViewInput['views'],
	id: string | null | undefined
): string | null {
	const candidate = typeof id === 'string' ? id.trim() : '';
	if (!candidate) return null;
	if (candidate === ALL_RECORDS_VIEW_ID) return ALL_RECORDS_VIEW_ID;
	return views.some((view) => view.id === candidate) ? candidate : null;
}

export function resolveInitialView(input: ResolveInitialViewInput): ResolvedView {
	const remembered = knownViewId(input.views, input.rememberedViewId);
	const dropRemembered =
		typeof input.rememberedViewId === 'string' &&
		input.rememberedViewId.trim() !== '' &&
		remembered === null;

	const fromUrl = knownViewId(input.views, input.urlViewId);
	if (fromUrl) return { viewId: fromUrl, source: 'url', dropRemembered };
	if (remembered) return { viewId: remembered, source: 'remembered', dropRemembered };

	const formDefault = input.views.find(viewIsDefault);
	if (formDefault) return { viewId: formDefault.id, source: 'form-default', dropRemembered };

	return { viewId: ALL_RECORDS_VIEW_ID, source: 'built-in', dropRemembered };
}

// Storage. Keys carry the user id so a shared browser never applies one person's choice to
// another; reads tolerate a missing, blocked or corrupted store (private mode, quota, old data).

export interface KeyValueStore {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
	removeItem(key: string): void;
}

export function browserStore(): KeyValueStore | null {
	try {
		return typeof localStorage === 'undefined' ? null : localStorage;
	} catch {
		return null;
	}
}

function lastViewKey(userId: string, formId: string): string {
	return `sv-forms-last-view:v1:${userId}:${formId}`;
}

export function readRememberedView(
	userId: string,
	formId: string,
	store: KeyValueStore | null = browserStore()
): string | null {
	if (!store || !userId || !formId) return null;
	try {
		const parsed: unknown = JSON.parse(store.getItem(lastViewKey(userId, formId)) ?? 'null');
		if (parsed && typeof parsed === 'object' && 'viewId' in parsed) {
			const viewId = (parsed as { viewId: unknown }).viewId;
			return typeof viewId === 'string' && viewId.trim() ? viewId : null;
		}
	} catch {
		/* Invalid JSON is the same as no memory. */
	}
	return null;
}

export function writeRememberedView(
	userId: string,
	formId: string,
	viewId: string,
	store: KeyValueStore | null = browserStore()
): void {
	if (!store || !userId || !formId || !viewId) return;
	try {
		store.setItem(
			lastViewKey(userId, formId),
			JSON.stringify({ viewId, at: new Date().toISOString() })
		);
	} catch {
		/* A full or blocked store only means the choice is not remembered. */
	}
}

export function forgetRememberedView(
	userId: string,
	formId: string,
	store: KeyValueStore | null = browserStore()
): void {
	if (!store || !userId || !formId) return;
	try {
		store.removeItem(lastViewKey(userId, formId));
	} catch {
		/* Nothing to forget. */
	}
}
