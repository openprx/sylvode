import {
	DEFAULT_DESTINATIONS,
	DESTINATIONS,
	SETTINGS_LABEL_KEY,
	SETTINGS_ROOT,
	railHref,
	type FlowAvailability,
	type ScopeKey
} from '../destinations';
import type { RouteContext } from '../route-map';
import {
	SHORTCUT_GLYPHS,
	railSelection,
	shortcutId,
	type RailSelection,
	type Shortcut,
	type ShortcutAvailability,
	type ShortcutKind
} from '../shortcuts.svelte';
import { projectHome } from '../../project/nav';
import type { LastKey } from '../storage';

/** `svelte-i18n`'s `$t` as the model sees it. */
export type Translate = (
	key: string,
	options?: { values?: Record<string, string | number> }
) => string;

/** The unread count prints "99+" above this. */
export const BADGE_CAP = 99;

export interface RailItemModel {
	kind: 'destination' | 'shortcut';
	/** Destination id, or the shortcut id. */
	id: string;
	label: string;
	/** Lucide glyph name. */
	glyph: string;
	/** null when the item is unavailable: it is then not a link. */
	href: string | null;
	selected: boolean;
	/** Translated reason, only for an unavailable item. */
	unavailableReason: string | null;
	/** Text of the count chip ("3", "99+"), or null. */
	badge: string | null;
	/** What assistive technology reads; differs from `label` only with a badge. */
	accessibleName: string;
	shortcutKind?: ShortcutKind;
}

export interface RailModel {
	destinations: RailItemModel[];
	shortcuts: RailItemModel[];
	/** Where the brand mark goes: the Home root of the scope. */
	homeHref: string;
	settings: { href: string; selected: boolean; label: string };
}

export interface RailModelInput {
	ctx: RouteContext;
	url: URL;
	scope: ScopeKey;
	flow: FlowAvailability;
	/** `ShellState.lastFor`. */
	lastFor: (key: LastKey, scope: ScopeKey) => string | undefined;
	shortcuts: readonly Shortcut[];
	availability: (id: string) => ShortcutAvailability;
	unread: number;
	t: Translate;
}

/** "3", "99+", or null when there is nothing unread. */
export function unreadBadge(count: number): string | null {
	if (!Number.isFinite(count) || count <= 0) return null;
	const whole = Math.floor(count);
	return whole > BADGE_CAP ? `${BADGE_CAP}+` : String(whole);
}

/** What the rail and top bar show: four fixed destinations, the user's shortcuts, Settings. */
export function buildRailModel(input: RailModelInput): RailModel {
	const { ctx, url, scope, flow, t } = input;
	const selection: RailSelection = railSelection(ctx, url, input.shortcuts);
	const badge = unreadBadge(input.unread);

	const destinations = DEFAULT_DESTINATIONS.map((id): RailItemModel => {
		const meta = DESTINATIONS[id];
		const label = t(meta.labelKey);
		const route = railHref(id, scope, input.lastFor(id, scope), flow);
		const count = id === 'home' ? badge : null;
		return {
			kind: 'destination',
			id,
			label,
			glyph: meta.glyph,
			href: route.available ? route.href : null,
			selected: selection.destination === id,
			unavailableReason: route.available ? null : t(route.reasonKey),
			badge: count,
			accessibleName: count ? t('shell.rail.homeUnread', { values: { label, count } }) : label
		};
	});

	const shortcuts = input.shortcuts.map((item): RailItemModel => {
		const availability = input.availability(item.id);
		const unavailable = availability.state === 'unavailable';
		const governanceOpen = item.kind === 'governance' && selection.column === 'governance';
		return {
			kind: 'shortcut',
			id: item.id,
			label: item.label,
			glyph: SHORTCUT_GLYPHS[item.kind],
			href: unavailable ? null : item.href,
			selected: !unavailable && (selection.shortcutId === item.id || governanceOpen),
			unavailableReason: availability.state === 'unavailable' ? t(availability.reason) : null,
			badge: null,
			accessibleName: item.label,
			shortcutKind: item.kind
		};
	});

	const home = destinations[0];
	return {
		destinations,
		shortcuts,
		homeHref: home.href ?? '/workspace',
		settings: {
			href: input.lastFor('settings', scope) ?? SETTINGS_ROOT,
			selected: selection.settings,
			label: t(SETTINGS_LABEL_KEY)
		}
	};
}

/**
 * The shortcut "Pin current page to rail" would add for a URL: a form, a project, a Flow page, or
 * the Governance centre. null on any other page. `label` is the page's own name; `projectTypeKey`
 * (when known) makes a project shortcut land where the project row does.
 */
export function currentPageShortcut(input: {
	ctx: RouteContext;
	url: URL;
	label: string;
	projectTypeKey?: string | null;
}): Shortcut | null {
	const { ctx, url } = input;
	const label = input.label.trim();
	if (!label || ctx.settings) return null;
	const workspaceId = ctx.workspaceId ?? undefined;
	if (ctx.formId && ctx.projectId) {
		return {
			id: shortcutId('form', ctx.formId),
			kind: 'form',
			label,
			href: url.pathname + url.search,
			workspaceId,
			projectId: ctx.projectId,
			formId: ctx.formId,
			objectId: ctx.formId
		};
	}
	if (ctx.projectId && ctx.workspaceId) {
		const href = input.projectTypeKey
			? projectHome(ctx.workspaceId, { id: ctx.projectId, type_key: input.projectTypeKey })
			: `/workspace/${encodeURIComponent(ctx.workspaceId)}/projects/${encodeURIComponent(ctx.projectId)}`;
		return {
			id: shortcutId('project', ctx.projectId),
			kind: 'project',
			label,
			href,
			workspaceId,
			projectId: ctx.projectId,
			objectId: ctx.projectId
		};
	}
	if (ctx.flowObjectId) {
		return {
			id: shortcutId('flow-page', ctx.flowObjectId),
			kind: 'flow-page',
			label,
			href: url.pathname,
			workspaceId,
			objectId: ctx.flowObjectId
		};
	}
	return null;
}

/** Activating the already selected destination toggles its column instead of navigating. */
export function isToggleTarget(item: Pick<RailItemModel, 'kind' | 'selected' | 'href'>): boolean {
	return item.kind === 'destination' && item.selected && item.href !== null;
}
