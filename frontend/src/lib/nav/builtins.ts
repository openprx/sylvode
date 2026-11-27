/** The built-in destinations a navigation entry can point at: label, glyph and href. */
import type { GlyphName } from '../shell/column/rows';

const enc = encodeURIComponent;

export interface BuiltinInfo {
	labelKey: string;
	glyph: GlyphName;
}

export const WORKSPACE_BUILTINS: Record<string, BuiltinInfo> = {
	'home.overview': { labelKey: 'shell.column.home.overview', glyph: 'house' },
	'home.notifications': { labelKey: 'shell.column.home.notifications', glyph: 'bell' },
	'home.my_work': { labelKey: 'shell.column.home.myWork', glyph: 'list-checks' },
	'home.recent': { labelKey: 'shell.column.home.recent', glyph: 'clock' },
	'workspace.milestones': { labelKey: 'milestones.title', glyph: 'flag' },
	'workspace.market': { labelKey: 'market.navTitle', glyph: 'puzzle' },
	'workspace.flow': { labelKey: 'shell.destinations.flow', glyph: 'workflow' },
	governance: { labelKey: 'shell.destinations.governance', glyph: 'landmark' }
};

export const PROJECT_BUILTINS: Record<string, BuiltinInfo> = {
	'project.overview': { labelKey: 'shell.projectNav.overview', glyph: 'layout-list' },
	'project.issues': { labelKey: 'shell.projectNav.issues', glyph: 'list-checks' },
	'project.board': { labelKey: 'shell.projectNav.board', glyph: 'kanban' },
	'project.cycles': { labelKey: 'shell.projectNav.cycles', glyph: 'repeat' },
	'project.forms': { labelKey: 'shell.projectNav.forms', glyph: 'table-2' },
	'project.apps': { labelKey: 'shell.projectNav.apps', glyph: 'app-window' },
	'project.settings': { labelKey: 'shell.projectNav.settings', glyph: 'settings' }
};

export function builtinInfo(page: string): BuiltinInfo | null {
	return WORKSPACE_BUILTINS[page] ?? PROJECT_BUILTINS[page] ?? null;
}

/** Where a workspace built-in goes; null for the ones that are not a single link (`home.recent`) or that depend on a flag. */
export function workspaceBuiltinHref(page: string, workspaceId: string): string | null {
	switch (page) {
		case 'home.overview':
			return '/workspace';
		case 'home.notifications':
			return '/inbox';
		case 'home.my_work':
			return '/my-work';
		case 'workspace.milestones':
			return `/workspace/${enc(workspaceId)}/milestones`;
		case 'workspace.market':
			return `/workspace/${enc(workspaceId)}/market`;
		case 'governance':
			return '/governance';
		default:
			return null;
	}
}

/** Where a project built-in goes. */
export function projectBuiltinHref(page: string, workspaceId: string, projectId: string): string {
	const base = `/workspace/${enc(workspaceId)}/projects/${enc(projectId)}`;
	switch (page) {
		case 'project.issues':
			return `${base}/issues`;
		case 'project.board':
			return `${base}/board`;
		case 'project.cycles':
			return `${base}/cycles`;
		case 'project.forms':
			return `${base}/forms`;
		case 'project.apps':
			return `${base}/apps`;
		case 'project.settings':
			return `${base}/settings`;
		default:
			return base;
	}
}
