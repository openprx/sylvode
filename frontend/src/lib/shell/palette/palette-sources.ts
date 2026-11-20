import { projectHome } from '../../project/nav';
import type { SearchResult } from '../../api/search';
import type { FlowSearchHit } from '../../api/flow';
import {
	PERSONAL_SCOPE,
	DEFAULT_DESTINATIONS,
	DESTINATIONS,
	railHref,
	SETTINGS_LABEL_KEY,
	SETTINGS_ROOT,
	type FlowAvailability,
	type ScopeKey
} from '../destinations';
import type { SettingsGroupModel } from '../settings-sections';
import type { LastKey, RecentEntry } from '../storage';
import type { Translate } from '../rail/rail-model';
import { MIN_SERVER_QUERY } from './palette-search';
import { rankLocal, type TextCollator } from './ranking';

export type PaletteGroupId =
	| 'goto'
	| 'recent'
	| 'projects'
	| 'forms'
	| 'flow'
	| 'issues'
	| 'actions'
	| 'results';

/** Groups in their fixed display order. */
export const GROUP_ORDER: readonly PaletteGroupId[] = [
	'goto',
	'recent',
	'projects',
	'forms',
	'flow',
	'issues',
	'actions',
	'results'
];

export const GROUP_LABEL_KEYS: Record<PaletteGroupId, string> = {
	goto: 'shell.palette.groups.goto',
	recent: 'shell.palette.groups.recent',
	projects: 'shell.palette.groups.projects',
	forms: 'shell.palette.groups.forms',
	flow: 'shell.palette.groups.flow',
	issues: 'shell.palette.groups.issues',
	actions: 'shell.palette.groups.actions',
	results: 'shell.palette.groups.results'
};

export const GROUP_LIMIT = 5;
export const TOTAL_LIMIT = 30;

export type PaletteKind =
	| 'destination'
	| 'settings'
	| 'workspace'
	| 'recent'
	| 'project'
	| 'form'
	| 'flow'
	| 'issue'
	| 'comment'
	| 'action'
	| 'search';

/** What the host does for an action entry. Nothing here writes or deletes by itself. */
export type PaletteActionId =
	| 'switch-workspace'
	| 'theme'
	| 'language'
	| 'pin-current'
	| 'edit-navigation'
	| 'open-settings'
	| 'sign-out';

export interface PaletteActionRef {
	id: PaletteActionId;
	/** Workspace id, theme or locale value. */
	arg?: string;
}

export interface PaletteEntry {
	/** Unique in the palette. */
	id: string;
	group: PaletteGroupId;
	kind: PaletteKind;
	title: string;
	/** Workspace or project, shown small under or beside the title. */
	context?: string;
	/** Action verb at the end of the row ("Open", "Switch", "Apply"). */
	verb: string;
	/** Lucide glyph name. */
	glyph: string;
	/** In-app path; set for entries that navigate. */
	href?: string;
	action?: PaletteActionRef;
	/** Larger is more recent. */
	recency?: number;
}

/** Snapshot of what the shell knows; the palette never reads stores itself. */
export interface PaletteContext {
	goto: readonly PaletteEntry[];
	recent: readonly PaletteEntry[];
	projects: readonly PaletteEntry[];
	forms: readonly PaletteEntry[];
	actions: readonly PaletteEntry[];
	/** The current workspace scope when Flow is on there; null otherwise. */
	flowWorkspace: { id: string; name: string } | null;
	/** Workspace a search hit falls back to when it carries none. */
	fallbackWorkspaceId: string | null;
}

export interface PaletteView {
	groups: Array<{ id: PaletteGroupId; label: string; entries: PaletteEntry[] }>;
	/** Every shown entry in display order: the keyboard walks this list. */
	entries: PaletteEntry[];
}

export interface PaletteInput {
	query: string;
	t: Translate;
	collator: TextCollator;
	goto: readonly PaletteEntry[];
	recent: readonly PaletteEntry[];
	projects: readonly PaletteEntry[];
	forms: readonly PaletteEntry[];
	actions: readonly PaletteEntry[];
	/** Server groups: null while nothing has arrived. They keep the server order. */
	flow: readonly PaletteEntry[] | null;
	issues: readonly PaletteEntry[] | null;
	/** A workspace scope with Flow on: the Flow pages group may show. */
	flowEnabled: boolean;
}

/** The "See all results" row, or null for a query too short to search. */
export function seeAllEntry(query: string, t: Translate): PaletteEntry | null {
	const q = query.trim();
	if ([...q].length < MIN_SERVER_QUERY) return null;
	return {
		id: 'results:all',
		group: 'results',
		kind: 'search',
		title: t('shell.palette.seeAll', { values: { query: q } }),
		verb: t('shell.palette.verbOpen'),
		glyph: 'search',
		href: `/search?q=${encodeURIComponent(q)}`
	};
}

/**
 * The groups for a query, in the fixed order. Local groups are ranked (tier, recency, alphabet);
 * server groups keep the server's order. At most 5 rows per group and 30 in total; an empty
 * group is not shown. Projects and Forms need one character, Flow pages and Issues two.
 */
export function buildPaletteView(input: PaletteInput): PaletteView {
	const q = input.query.trim();
	const length = [...q].length;
	const local = (items: readonly PaletteEntry[]) => rankLocal(items, q, input.collator);

	const byGroup: Record<PaletteGroupId, readonly PaletteEntry[]> = {
		goto: local(input.goto),
		recent: local(input.recent),
		projects: length >= 1 ? local(input.projects) : [],
		forms: length >= 1 ? local(input.forms) : [],
		flow: length >= MIN_SERVER_QUERY && input.flowEnabled ? (input.flow ?? []) : [],
		issues: length >= MIN_SERVER_QUERY ? (input.issues ?? []) : [],
		actions: local(input.actions),
		results: []
	};
	const all = seeAllEntry(q, input.t);
	if (all) byGroup.results = [all];

	const groups: PaletteView['groups'] = [];
	const entries: PaletteEntry[] = [];
	for (const id of GROUP_ORDER) {
		const room = TOTAL_LIMIT - entries.length;
		if (room <= 0) break;
		const shown = byGroup[id].slice(0, Math.min(GROUP_LIMIT, room));
		if (shown.length === 0) continue;
		groups.push({ id, label: input.t(GROUP_LABEL_KEYS[id]), entries: shown });
		entries.push(...shown);
	}
	return { groups, entries };
}

// --- entry builders --------------------------------------------------------------------------

export interface GotoInput {
	t: Translate;
	scope: ScopeKey;
	flow: FlowAvailability;
	lastFor: (key: LastKey, scope: ScopeKey) => string | undefined;
	/** The settings sections the viewer may see (`settingsGroups`). */
	settingsGroups: readonly SettingsGroupModel[];
}

/** "Go to": the destinations that are reachable, Settings, each settings section, All workspaces. */
export function gotoEntries(input: GotoInput): PaletteEntry[] {
	const { t } = input;
	const verb = t('shell.palette.verbOpen');
	const out: PaletteEntry[] = [];
	for (const id of DEFAULT_DESTINATIONS) {
		const route = railHref(id, input.scope, input.lastFor(id, input.scope), input.flow);
		if (!route.available) continue;
		out.push({
			id: `goto:${id}`,
			group: 'goto',
			kind: 'destination',
			title: t(DESTINATIONS[id].labelKey),
			verb,
			glyph: DESTINATIONS[id].glyph,
			href: route.href
		});
	}
	out.push({
		id: 'goto:settings',
		group: 'goto',
		kind: 'settings',
		title: t(SETTINGS_LABEL_KEY),
		verb,
		glyph: 'settings',
		href: input.lastFor('settings', input.scope) ?? SETTINGS_ROOT
	});
	for (const group of input.settingsGroups) {
		for (const section of group.sections) {
			out.push({
				id: `goto:settings:${group.id}:${section.id}`,
				group: 'goto',
				kind: 'settings',
				title: t(section.labelKey),
				context: group.caption ? `${t(group.labelKey)} · ${group.caption}` : t(group.labelKey),
				verb,
				glyph: 'settings',
				href: section.href
			});
		}
	}
	out.push({
		id: 'goto:all-workspaces',
		group: 'goto',
		kind: 'workspace',
		title: t('shell.palette.allWorkspaces'),
		verb,
		glyph: 'folder-kanban',
		href: '/workspace'
	});
	return out;
}

/** Recent task resources; the most recent first, so the first has the highest recency. */
export function recentEntries(recent: readonly RecentEntry[], t: Translate): PaletteEntry[] {
	return recent.map((r, index) => ({
		id: `recent:${r.href}`,
		group: 'recent',
		kind: 'recent',
		title: r.title,
		verb: t('shell.palette.verbOpen'),
		glyph: 'file-text',
		href: r.href,
		recency: recent.length - index
	}));
}

export interface ProjectLike {
	id: string;
	name: string;
	workspace_id: string;
	workspace_name: string;
	type_key: string;
}

/** Projects of every workspace; `lastActive` maps an href to the time its task tab was last active. */
export function projectEntries(
	projects: readonly ProjectLike[],
	t: Translate,
	lastActive: (href: string) => number = () => 0
): PaletteEntry[] {
	return projects.map((p) => {
		const href = projectHome(p.workspace_id, p);
		return {
			id: `project:${p.id}`,
			group: 'projects',
			kind: 'project',
			title: p.name,
			context: p.workspace_name,
			verb: t('shell.palette.verbOpen'),
			glyph: 'folder',
			href,
			recency: lastActive(href)
		};
	});
}

export interface FormLike {
	id: string;
	name: string;
	projectId: string;
	projectName: string;
	workspaceId: string;
}

/** Forms already cached by the project tree: there is no cross-project endpoint. */
export function formEntries(
	forms: readonly FormLike[],
	t: Translate,
	lastActive: (href: string) => number = () => 0
): PaletteEntry[] {
	return forms.map((f) => {
		const href = `/workspace/${encodeURIComponent(f.workspaceId)}/projects/${encodeURIComponent(f.projectId)}/forms?form=${encodeURIComponent(f.id)}`;
		return {
			id: `form:${f.id}`,
			group: 'forms',
			kind: 'form',
			title: f.name,
			context: f.projectName,
			verb: t('shell.palette.verbOpen'),
			glyph: 'table-2',
			href,
			recency: lastActive(href)
		};
	});
}

export interface ActionsInput {
	t: Translate;
	scope: ScopeKey;
	workspaces: ReadonlyArray<{ id: string; name: string }>;
	theme: 'light' | 'dark' | 'system';
	locale: 'zh' | 'en';
	/** The page "Pin current page to rail" would pin: its label, or null when there is none. */
	pinnable: string | null;
}

const LOCALE_NAMES: Record<'zh' | 'en', string> = { zh: '中文', en: 'English' };

/** Actions. None writes or deletes anything itself; the host decides what each one does. */
export function actionEntries(input: ActionsInput): PaletteEntry[] {
	const { t } = input;
	const apply = t('shell.palette.verbApply');
	const out: PaletteEntry[] = [];
	for (const w of input.workspaces) {
		if (w.id === input.scope) continue;
		out.push({
			id: `action:workspace:${w.id}`,
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.switchWorkspace', { values: { name: w.name } }),
			verb: t('shell.palette.verbSwitch'),
			glyph: 'folder-kanban',
			action: { id: 'switch-workspace', arg: w.id }
		});
	}
	if (input.scope !== PERSONAL_SCOPE)
		out.push({
			id: 'action:workspace:personal',
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.switchPersonal'),
			verb: t('shell.palette.verbSwitch'),
			glyph: 'house',
			action: { id: 'switch-workspace', arg: PERSONAL_SCOPE }
		});
	for (const value of ['light', 'dark', 'system'] as const) {
		if (value === input.theme) continue;
		out.push({
			id: `action:theme:${value}`,
			group: 'actions',
			kind: 'action',
			title: t(`shell.palette.theme.${value}`),
			verb: apply,
			glyph: 'settings',
			action: { id: 'theme', arg: value }
		});
	}
	for (const value of ['zh', 'en'] as const) {
		if (value === input.locale) continue;
		out.push({
			id: `action:language:${value}`,
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.language', { values: { name: LOCALE_NAMES[value] } }),
			verb: apply,
			glyph: 'settings',
			action: { id: 'language', arg: value }
		});
	}
	if (input.pinnable)
		out.push({
			id: 'action:pin-current',
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.pinCurrent'),
			context: input.pinnable,
			verb: apply,
			glyph: 'layout-list',
			action: { id: 'pin-current' }
		});
	out.push(
		{
			id: 'action:edit-navigation',
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.editNavigation'),
			verb: t('shell.palette.verbOpen'),
			glyph: 'layout-list',
			action: { id: 'edit-navigation' }
		},
		{
			id: 'action:open-settings',
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.openSettings'),
			verb: t('shell.palette.verbOpen'),
			glyph: 'settings',
			action: { id: 'open-settings' }
		},
		{
			id: 'action:sign-out',
			group: 'actions',
			kind: 'action',
			title: t('shell.palette.signOut'),
			verb: t('shell.palette.verbSignOut'),
			glyph: 'settings',
			action: { id: 'sign-out' }
		}
	);
	return out;
}

function cleanId(value: unknown): string {
	if (typeof value !== 'string') return '';
	const next = value.trim();
	return !next || next === 'undefined' || next === 'null' ? '' : next;
}

/** Where a search hit opens; null when the hit does not say enough (no workspace or project). */
export function searchResultHref(
	item: SearchResult,
	fallbackWorkspaceId: string | null
): string | null {
	const raw = item as unknown as Record<string, unknown>;
	const workspaceId = cleanId(raw.workspace_id ?? raw.workspaceId) || cleanId(fallbackWorkspaceId);
	if (!workspaceId) return null;
	const base = `/workspace/${encodeURIComponent(workspaceId)}`;
	if (item.type === 'project') {
		const projectId = cleanId(item.id);
		return projectId ? `${base}/projects/${encodeURIComponent(projectId)}` : null;
	}
	const projectId = cleanId(raw.project_id ?? raw.projectId);
	const issueId = cleanId(item.type === 'issue' ? item.id : (raw.issue_id ?? raw.issueId));
	return projectId && issueId
		? `${base}/projects/${encodeURIComponent(projectId)}/issues/${encodeURIComponent(issueId)}`
		: null;
}

function preview(text: string | undefined | null, max = 80): string {
	const plain = (text ?? '').replace(/\s+/g, ' ').trim();
	return plain.length > max ? `${plain.slice(0, max)}...` : plain;
}

/**
 * Issues and comments from the global search, in server order. Project hits are left out (the
 * Projects group already lists every project); a hit without a usable address is skipped.
 */
export function issueEntries(
	results: readonly SearchResult[],
	fallbackWorkspaceId: string | null,
	t: Translate
): PaletteEntry[] {
	const out: PaletteEntry[] = [];
	for (const item of results) {
		if (item.type === 'project') continue;
		const href = searchResultHref(item, fallbackWorkspaceId);
		if (!href) continue;
		out.push({
			id: `${item.type}:${item.id}`,
			group: 'issues',
			kind: item.type,
			title: item.type === 'issue' ? item.title : t('shell.palette.comment'),
			context:
				item.type === 'issue' ? preview(item.description, 60) || undefined : preview(item.body),
			verb: t('shell.palette.verbOpen'),
			glyph: item.type === 'issue' ? 'layout-list' : 'file-text',
			href
		});
	}
	return out;
}

/** Flow pages of the current workspace, in server order. */
export function flowEntries(
	items: readonly FlowSearchHit[],
	workspace: { id: string; name: string },
	t: Translate
): PaletteEntry[] {
	return items.map((hit) => ({
		id: `flow:${hit.object.id}`,
		group: 'flow',
		kind: 'flow',
		title: hit.object.title || t('shell.palette.untitled'),
		context: workspace.name,
		verb: t('shell.palette.verbOpen'),
		glyph: 'workflow',
		href: `/workspace/${encodeURIComponent(workspace.id)}/flow/${encodeURIComponent(hit.object.id)}`
	}));
}
