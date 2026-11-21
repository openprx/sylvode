/** Scope switcher rules (contract 2.4.1): the menu's workspaces, the label, where a change goes. */
import {
	PERSONAL_SCOPE,
	destinationRoot,
	type FlowAvailability,
	type ScopeKey
} from '../destinations';
import type { RouteContext } from '../route-map';

export interface WorkspaceRef {
	id: string;
	name: string;
}

/** Workspaces sorted by name, locale-aware; the input is not changed. */
export function sortWorkspaces(
	workspaces: readonly WorkspaceRef[],
	locale?: string
): WorkspaceRef[] {
	const collator = new Intl.Collator(locale);
	return [...workspaces].sort(
		(a, b) => collator.compare(a.name, b.name) || a.id.localeCompare(b.id)
	);
}

/** The name on the trigger: the personal label, the workspace name, or `fallback` while unknown. */
export function scopeLabel(
	scope: ScopeKey,
	workspaces: readonly WorkspaceRef[],
	labels: { personal: string; fallback: string }
): string {
	if (scope === PERSONAL_SCOPE) return labels.personal;
	return workspaces.find((w) => w.id === scope)?.name ?? labels.fallback;
}

/** Path of a workspace settings section under a workspace. */
const WORKSPACE_SECTION_PATHS: Record<string, string> = {
	general: '/settings',
	members: '/members',
	webhooks: '/webhooks',
	connections: '/connections',
	workflows: '/workflows',
	flow: '/settings/flow'
};

export interface ScopeChangeInput {
	ctx: RouteContext;
	/** The scope that was chosen. */
	scope: ScopeKey;
	flow: FlowAvailability;
	/** Viewer is owner or admin of the chosen workspace (settings only; false when unknown). */
	adminInNewScope?: boolean;
}

/**
 * Where choosing a scope goes, or null to stay on the current URL. A URL that names another
 * workspace cannot stay: the shell would adopt that workspace as the scope again.
 */
export function scopeChangeTarget(input: ScopeChangeInput): string | null {
	const { ctx, scope, flow } = input;
	const personal = scope === PERSONAL_SCOPE;
	const ws = encodeURIComponent(scope);

	if (ctx.settings) {
		if (ctx.settings.group !== 'workspace' && ctx.settings.group !== 'project') return null;
		if (personal) return '/settings';
		const suffix = WORKSPACE_SECTION_PATHS[ctx.settings.section];
		return input.adminInNewScope && suffix
			? `/workspace/${ws}${suffix}`
			: `/workspace/${ws}/settings`;
	}
	switch (ctx.destination) {
		case 'projects':
			return personal ? '/workspace' : `/workspace/${ws}/projects`;
		case 'flow': {
			const root = destinationRoot('flow', scope, flow);
			return root.available ? root.href : null;
		}
		case 'home':
			if (ctx.workspaceId && ctx.workspaceId !== scope)
				return personal ? '/workspace' : `/workspace/${ws}`;
			return null;
		case 'agents':
			return ctx.workspaceId && ctx.workspaceId !== scope ? '/agents' : null;
		default:
			return null;
	}
}
