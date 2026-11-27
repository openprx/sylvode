/**
 * The project tree of the secondary column, as data. Pure: no requests, no stores. The column
 * fetches forms and the apps count lazily and passes what it has; `null` means "not loaded yet".
 */

import { projectChildren, type AppRef } from '../nav/rows';
import type { ResolvedNav } from '../nav/types';

export type TreeKind =
	| 'project'
	| 'overview'
	| 'forms'
	| 'form'
	| 'all-forms'
	| 'apps'
	| 'app'
	| 'link'
	| 'caption'
	| 'issues'
	| 'board'
	| 'cycles'
	| 'settings';

export interface TreeNode {
	kind: TreeKind;
	/** Unique in the tree: the object id for projects and forms, `${kind}:${projectId}` for rows. */
	id: string;
	label: string;
	href: string;
	children?: TreeNode[];
	/** Children exist but are fetched on first expansion. */
	lazy?: boolean;
	/** Glyph name overriding the kind's default (navigation entries with an icon). */
	glyph?: string;
	/** An external link: opens in a new tab. */
	external?: boolean;
}

export type NavTranslate = (key: string) => string;

/** i18n keys of the fixed rows (messages live in `lib/shell/locale.*.json`). */
export const PROJECT_NAV_KEYS = {
	overview: 'shell.projectNav.overview',
	issues: 'shell.projectNav.issues',
	board: 'shell.projectNav.board',
	cycles: 'shell.projectNav.cycles',
	forms: 'shell.projectNav.forms',
	allForms: 'shell.projectNav.allForms',
	apps: 'shell.projectNav.apps',
	settings: 'shell.projectNav.settings',
	pluginsSection: 'shell.sections.projectPlugins',
	executionSection: 'execution.connection.title',
	inboundSourcesSection: 'inbound.sources.title',
	alertRulesSection: 'inbound.rules.title'
} as const;

/** Type key of the form-centred project type: its overview is the forms page. */
export const CUSTOM_FORM_TYPE = 'custom_form';
/** More forms than this in a project adds an "All forms" row. */
export const FORMS_IN_TREE = 100;

const identity: NavTranslate = (key) => key;

function projectBase(workspaceId: string, projectId: string): string {
	return `/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}`;
}

export interface ProjectTreeInput {
	workspaceId: string;
	project: { id: string; name: string; type_key: string };
	/** Owner or admin of the workspace (the role tracker's rule). */
	isWorkspaceAdmin: boolean;
	/** null = not loaded yet. */
	appsCount: number | null;
	forms: Array<{ id: string; name: string }> | null;
	formsTotal: number | null;
	/** Resolves the fixed rows' labels; defaults to returning the message key. */
	translate?: NavTranslate;
	/** The project's resolved navigation; the built-in rows apply while it is null or absent. */
	menu?: ResolvedNav | null;
	/** Apps of the project for the Apps row's children; null while discovery has not answered. */
	apps?: AppRef[] | null;
	locale?: string;
}

/** Where the project row itself goes: forms for `custom_form`, else the overview. */
export function projectHome(
	workspaceId: string,
	project: { id: string; type_key: string }
): string {
	const base = projectBase(workspaceId, project.id);
	return project.type_key === CUSTOM_FORM_TYPE ? `${base}/forms` : base;
}

export interface SettingsSectionEntry {
	id: string;
	label: string;
	labelKey: string;
	href: string;
}

/**
 * Project settings sections for a type. Empty unless the viewer is an owner or admin. `base` is
 * the project's own URL (without a trailing slash) the hrefs hang from.
 */
export function settingsSections(
	typeKey: string,
	isAdmin: boolean,
	options: { base?: string; translate?: NavTranslate } = {}
): SettingsSectionEntry[] {
	if (!isAdmin) return [];
	void typeKey; // every type has Plugins, Execution, Inbound sources and Alert rules; External API, Public forms and Templates join per type later
	const base = options.base ?? '';
	const t = options.translate ?? identity;
	return [
		{
			id: 'plugins',
			label: t(PROJECT_NAV_KEYS.pluginsSection),
			labelKey: PROJECT_NAV_KEYS.pluginsSection,
			href: `${base}/settings/plugins`
		},
		{
			id: 'execution',
			label: t(PROJECT_NAV_KEYS.executionSection),
			labelKey: PROJECT_NAV_KEYS.executionSection,
			href: `${base}/settings/execution`
		},
		{
			id: 'inbound-sources',
			label: t(PROJECT_NAV_KEYS.inboundSourcesSection),
			labelKey: PROJECT_NAV_KEYS.inboundSourcesSection,
			href: `${base}/settings/inbound-sources`
		},
		{
			id: 'alert-rules',
			label: t(PROJECT_NAV_KEYS.alertRulesSection),
			labelKey: PROJECT_NAV_KEYS.alertRulesSection,
			href: `${base}/settings/alert-rules`
		}
	];
}

/** Where `…/settings` lands. */
export function settingsLanding(workspaceId: string, projectId: string, typeKey: string): string {
	const base = projectBase(workspaceId, projectId);
	return settingsSections(typeKey, true, { base })[0].href;
}

export function projectTree(input: ProjectTreeInput): TreeNode {
	const { workspaceId, project, isWorkspaceAdmin, appsCount, forms, formsTotal } = input;
	const t = input.translate ?? identity;
	const base = projectBase(workspaceId, project.id);
	const row = (kind: TreeKind, key: string, href: string): TreeNode => ({
		kind,
		id: `${kind}:${project.id}`,
		label: t(key),
		href
	});

	if (input.menu) {
		return {
			kind: 'project',
			id: project.id,
			label: project.name,
			href: projectHome(workspaceId, project),
			children: projectChildren(input.menu, {
				workspaceId,
				projectId: project.id,
				locale: input.locale ?? 'en',
				tr: t,
				isWorkspaceAdmin,
				apps: input.apps ?? null,
				forms,
				formsTotal,
				formsInTree: FORMS_IN_TREE
			})
		};
	}

	const children: TreeNode[] = [];
	if (project.type_key !== CUSTOM_FORM_TYPE) {
		children.push(row('overview', PROJECT_NAV_KEYS.overview, base));
		children.push(row('issues', PROJECT_NAV_KEYS.issues, `${base}/issues`));
		children.push(row('board', PROJECT_NAV_KEYS.board, `${base}/board`));
		children.push(row('cycles', PROJECT_NAV_KEYS.cycles, `${base}/cycles`));
	}

	const formsNode = row('forms', PROJECT_NAV_KEYS.forms, `${base}/forms`);
	if (forms === null) formsNode.lazy = true;
	else {
		formsNode.children = forms.map((form) => ({
			kind: 'form' as const,
			id: form.id,
			label: form.name,
			href: `${base}/forms?form=${encodeURIComponent(form.id)}`
		}));
		if (formsTotal !== null && formsTotal > FORMS_IN_TREE)
			formsNode.children.push(row('all-forms', PROJECT_NAV_KEYS.allForms, `${base}/forms`));
	}
	children.push(formsNode);

	if ((appsCount ?? 0) > 0 || (input.apps?.length ?? 0) > 0 || isWorkspaceAdmin) {
		const appsNode = row('apps', PROJECT_NAV_KEYS.apps, `${base}/apps`);
		if (input.apps && input.apps.length > 0)
			appsNode.children = input.apps.map((app) => ({
				kind: 'app' as const,
				id: `app:${project.id}:${app.key}:${app.route}`,
				label: app.label,
				href: `${base}/apps/${encodeURIComponent(app.key)}/${encodeURIComponent(app.route)}`
			}));
		children.push(appsNode);
	}
	if (isWorkspaceAdmin)
		children.push(row('settings', PROJECT_NAV_KEYS.settings, `${base}/settings`));

	return {
		kind: 'project',
		id: project.id,
		label: project.name,
		href: projectHome(workspaceId, project),
		children
	};
}
