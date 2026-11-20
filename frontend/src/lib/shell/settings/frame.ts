/**
 * Settings mode frame (contract 4): who sees which group, the model the section list renders,
 * and where "Close settings" goes. Pure: the groups themselves come from `settings-sections.ts`.
 */
import type { User } from '#lib/api/auth.js';
import { isAdminUser } from '#lib/utils/auth.js';
import { showWorkspaceAdminLinks } from '#lib/workspace/role.js';
import type { WorkspaceRole } from '#lib/workspace/home.js';
import { PERSONAL_SCOPE, type ScopeKey } from '../destinations';
import type { RouteContext } from '../route-map';
import {
	sectionFor,
	settingsGroups,
	type SettingsGroupModel,
	type SettingsSectionModel,
	type SettingsViewer
} from '../settings-sections';

export interface SettingsViewerInput {
	user: User | null;
	/** The user's member role in the scope's workspace; null when unknown or not a member. */
	workspaceRole: WorkspaceRole | null;
	/** Personal Flow is available (the Flow packages link). */
	personalFlow: boolean;
	scope: ScopeKey;
	workspaces: ReadonlyArray<{ id: string; name: string }>;
	/** Current route, to detect project settings. */
	ctx: RouteContext;
	projects: ReadonlyArray<{ id: string; name: string; type_key: string }>;
}

/** The viewer `settingsGroups()` takes, from what the shell knows. */
export function buildSettingsViewer(input: SettingsViewerInput): SettingsViewer {
	const instanceAdmin = isAdminUser(input.user);
	const workspaceAdmin = showWorkspaceAdminLinks({
		workspaceRole: input.workspaceRole,
		instanceAdmin
	});
	const workspace =
		input.scope === PERSONAL_SCOPE
			? null
			: {
					id: input.scope,
					name: input.workspaces.find((w) => w.id === input.scope)?.name ?? ''
				};
	let project: SettingsViewer['project'] = null;
	if (input.ctx.settings?.group === 'project' && input.ctx.workspaceId && input.ctx.projectId) {
		const found = input.projects.find((p) => p.id === input.ctx.projectId);
		project = {
			workspaceId: input.ctx.workspaceId,
			id: input.ctx.projectId,
			name: found?.name ?? '',
			typeKey: found?.type_key ?? ''
		};
	}
	return {
		instanceAdmin,
		workspaceAdmin,
		personalFlow: input.personalFlow,
		workspace,
		project
	};
}

export interface SettingsNavSection extends SettingsSectionModel {
	selected: boolean;
}
export interface SettingsNavGroup extends Omit<SettingsGroupModel, 'sections'> {
	sections: SettingsNavSection[];
}

/** The groups to render with the current section marked; empty groups stay (the personal prompt). */
export function settingsNavModel(viewer: SettingsViewer, ctx: RouteContext): SettingsNavGroup[] {
	const groups = settingsGroups(viewer);
	const current = sectionFor(ctx, groups);
	return groups.map((group) => ({
		...group,
		sections: group.sections.map((section) => ({
			...section,
			selected: current?.group.id === group.id && current.section.id === section.id
		}))
	}));
}

/** Where "Close settings" goes: the stored return URL, else the Home root. */
export function closeTarget(
	returnHref: string | null | undefined,
	homeRoot = '/workspace'
): string {
	return returnHref || homeRoot;
}
