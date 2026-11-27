/**
 * The resolved navigation as rows: pure, no requests, no stores. The server decides what a viewer
 * may see (`sv.nav.resolved.v1`); this turns it into what the columns render and adds what only
 * the client knows (built-in hrefs, the lazy forms and apps children, the Flow flag).
 */
import { destinationRoot, type FlowAvailability } from '../shell/destinations';
import type { GlyphName, NavRow } from '../shell/column/rows';
import type { TreeNode } from '../project/nav';
import { builtinInfo, projectBuiltinHref, workspaceBuiltinHref } from './builtins';
import {
	PINNED_SECTION,
	type LocaleLabels,
	type ResolvedEntry,
	type ResolvedNav,
	type ResolvedSection
} from './types';

export type Translate = (key: string, values?: Record<string, string | number>) => string;

const enc = encodeURIComponent;

/** The text for a locale: the locale itself, its language, English, then any. */
export function pickLabel(labels: LocaleLabels | null | undefined, locale: string): string | null {
	if (!labels) return null;
	const lang = locale.split('-')[0];
	return labels[locale] ?? labels[lang] ?? labels.en ?? Object.values(labels)[0] ?? null;
}

const SECTION_KEYS: Record<string, string> = {
	[PINNED_SECTION]: 'nav.sections.pinned',
	recent: 'shell.column.home.recent',
	more: 'nav.sections.more'
};

/** A section's caption: its own label, else the built-in caption of the default sections. */
export function sectionCaption(
	section: Pick<ResolvedSection, 'id' | 'label'>,
	locale: string,
	tr: Translate
): string | null {
	return (
		pickLabel(section.label, locale) ??
		(SECTION_KEYS[section.id] ? tr(SECTION_KEYS[section.id]) : null)
	);
}

/** A glyph the column knows, else the fallback. */
export function glyphOf(icon: string | null, fallback: GlyphName): GlyphName {
	const known: readonly string[] = [
		'house',
		'bell',
		'list-checks',
		'clock',
		'folder',
		'folder-kanban',
		'layout-list',
		'kanban',
		'repeat',
		'table-2',
		'file-text',
		'app-window',
		'settings',
		'workflow',
		'bot',
		'sparkles',
		'landmark',
		'flag',
		'users',
		'briefcase',
		'link',
		'external-link',
		'star',
		'layout-template',
		'globe',
		'puzzle'
	];
	return icon && known.includes(icon) ? (icon as GlyphName) : fallback;
}

const KIND_GLYPH: Record<ResolvedEntry['kind'], GlyphName> = {
	builtin: 'link',
	project: 'folder',
	form: 'file-text',
	flow_page: 'workflow',
	layout_page: 'layout-template',
	plugin_app: 'puzzle',
	external: 'external-link'
};

/** An app of a project (`project.view` discovery), as the navigation needs it. */
export interface AppRef {
	key: string;
	route: string;
	label: string;
}

// --- workspace (Home column) ---

export interface WorkspaceRowsInput {
	workspaceId: string;
	locale: string;
	tr: Translate;
	unread: number;
	flow: FlowAvailability;
	/** Rows of the `home.recent` entry (already built from the shell's recent list). */
	recent: NavRow[];
}

export interface RowGroup {
	id: string;
	caption: string | null;
	collapsed: boolean;
	rows: NavRow[];
}

function linkRow(entry: ResolvedEntry, label: string, href: string, glyph: GlyphName): NavRow {
	const row: NavRow = {
		id: `nav:${entry.id}`,
		label,
		href,
		glyph,
		level: 0,
		parentId: null,
		attrs: { 'data-home-row': entry.id, 'data-nav-entry': entry.id }
	};
	if (entry.kind === 'external') {
		row.external = true;
		row.selectable = false;
	}
	return row;
}

function workspaceRowsOf(entry: ResolvedEntry, input: WorkspaceRowsInput): NavRow[] {
	const { tr, locale } = input;
	if (entry.target.kind === 'builtin') {
		const page = entry.target.page;
		const info = builtinInfo(page);
		if (!info) return [];
		if (page === 'home.recent') return input.recent;
		let href = workspaceBuiltinHref(page, input.workspaceId);
		if (page === 'workspace.flow') {
			const root = destinationRoot('flow', input.workspaceId, input.flow);
			if (!root.available) return [];
			href = root.href;
		}
		if (href === null) return [];
		const row = linkRow(
			entry,
			pickLabel(entry.label, locale) ?? tr(info.labelKey),
			href,
			glyphOf(entry.icon, info.glyph)
		);
		if (page === 'home.notifications' && input.unread > 0) row.count = input.unread;
		return [row];
	}
	if (!entry.href) return [];
	const label = pickLabel(entry.label, locale) ?? entry.name ?? entry.href;
	return [linkRow(entry, label, entry.href, glyphOf(entry.icon, KIND_GLYPH[entry.kind]))];
}

/** The groups of the Home column from the workspace menu; empty groups are left out. */
export function workspaceGroups(menu: ResolvedNav, input: WorkspaceRowsInput): RowGroup[] {
	return menu.sections
		.map((section) => ({
			id: section.id,
			caption: sectionCaption(section, input.locale, input.tr),
			collapsed: section.collapsed,
			rows: section.entries.flatMap((entry) => workspaceRowsOf(entry, input))
		}))
		.filter((group) => group.rows.length > 0);
}

// --- project (children of the project row) ---

export interface ProjectChildrenInput {
	workspaceId: string;
	projectId: string;
	locale: string;
	tr: Translate;
	isWorkspaceAdmin: boolean;
	/** Apps of the project; null while the discovery has not answered. */
	apps: AppRef[] | null;
	/** Forms of the project for the lazy `project.forms` children; null = not loaded. */
	forms: Array<{ id: string; name: string }> | null;
	formsTotal: number | null;
	formsInTree: number;
}

function projectNode(entry: ResolvedEntry, input: ProjectChildrenInput): TreeNode | null {
	const { tr, locale, workspaceId, projectId } = input;
	const base = `/workspace/${enc(workspaceId)}/projects/${enc(projectId)}`;
	const own = pickLabel(entry.label, locale);
	const target = entry.target;
	if (target.kind === 'builtin') {
		const info = builtinInfo(target.page);
		if (!info) return null;
		const node: TreeNode = {
			kind: 'link',
			id: `nav:${entry.id}:${projectId}`,
			label: own ?? tr(info.labelKey),
			href: projectBuiltinHref(target.page, workspaceId, projectId),
			glyph: glyphOf(entry.icon, info.glyph)
		};
		if (target.page === 'project.forms') {
			node.kind = 'forms';
			node.id = `forms:${projectId}`;
			if (input.forms === null) node.lazy = true;
			else {
				node.children = input.forms.map((form) => ({
					kind: 'form' as const,
					id: form.id,
					label: form.name,
					href: `${base}/forms?form=${enc(form.id)}`
				}));
				if (input.formsTotal !== null && input.formsTotal > input.formsInTree)
					node.children.push({
						kind: 'all-forms',
						id: `all-forms:${projectId}`,
						label: tr('shell.projectNav.allForms'),
						href: `${base}/forms`
					});
			}
		} else if (target.page === 'project.apps') {
			node.kind = 'apps';
			node.id = `apps:${projectId}`;
			// The row shows for people who can manage apps, or when the project has some.
			if ((input.apps?.length ?? 0) === 0 && !input.isWorkspaceAdmin) return null;
			if (input.apps && input.apps.length > 0)
				node.children = input.apps.map((app) => ({
					kind: 'app' as const,
					id: `app:${projectId}:${app.key}:${app.route}`,
					label: app.label,
					href: `${base}/apps/${enc(app.key)}/${enc(app.route)}`
				}));
		}
		return node;
	}
	if (target.kind === 'plugin_app') {
		const app = input.apps?.find((a) => a.key === target.package_key && a.route === target.route);
		if (!app) return null;
		return {
			kind: 'link',
			id: `nav:${entry.id}:${projectId}`,
			label: own ?? app.label,
			href: `${base}/apps/${enc(app.key)}/${enc(app.route)}`,
			glyph: glyphOf(entry.icon, 'puzzle')
		};
	}
	if (!entry.href) return null;
	const node: TreeNode = {
		kind: 'link',
		id: `nav:${entry.id}:${projectId}`,
		label: own ?? entry.name ?? entry.href,
		href: entry.href,
		glyph: glyphOf(entry.icon, KIND_GLYPH[entry.kind])
	};
	if (entry.kind === 'external') node.external = true;
	return node;
}

/** The children of a project row from its menu: section captions and entries, in order. */
export function projectChildren(menu: ResolvedNav, input: ProjectChildrenInput): TreeNode[] {
	const nodes: TreeNode[] = [];
	for (const section of menu.sections) {
		const entries = section.entries
			.map((entry) => projectNode(entry, input))
			.filter((node): node is TreeNode => node !== null);
		if (entries.length === 0) continue;
		const caption = section.id === 'main' ? null : sectionCaption(section, input.locale, input.tr);
		if (caption)
			nodes.push({
				kind: 'caption',
				id: `caption:${section.id}:${input.projectId}`,
				label: caption,
				href: ''
			});
		nodes.push(...entries);
	}
	return nodes;
}
