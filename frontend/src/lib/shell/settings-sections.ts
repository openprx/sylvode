import { settingsSections } from '../project/nav';
import type { RouteContext, SettingsGroup } from './route-map';

export interface SettingsViewer {
	/** Instance admin (`isAdminUser`). */
	instanceAdmin: boolean;
	/** Owner or admin of `workspace` (the role tracker's rule). */
	workspaceAdmin: boolean;
	/** Personal Flow is available to the user. */
	personalFlow: boolean;
	/** The current scope's workspace; null in the personal scope. */
	workspace: { id: string; name: string } | null;
	/** Set on project settings routes: the section list then shows the project group only. */
	project?: { workspaceId: string; id: string; name: string; typeKey: string } | null;
}

export interface SettingsSectionModel {
	id: string;
	/** i18n key of the label. */
	labelKey: string;
	href: string;
	/** Leaves settings mode (an outbound link). */
	external?: boolean;
}

export interface SettingsGroupModel {
	id: SettingsGroup;
	/** i18n key of the group caption. */
	labelKey: string;
	/** Name shown next to the caption (workspace or project name). */
	caption?: string;
	/** Personal scope: the group is one row that opens the scope switcher. */
	chooseWorkspace?: boolean;
	sections: SettingsSectionModel[];
}

const GROUP_KEYS: Record<SettingsGroup, string> = {
	account: 'shell.sections.groups.account',
	workspace: 'shell.sections.groups.workspace',
	instance: 'shell.sections.groups.instance',
	project: 'shell.sections.groups.project'
};

/** The groups and sections the viewer may see, in display order. */
export function settingsGroups(viewer: SettingsViewer): SettingsGroupModel[] {
	if (viewer.project) {
		const p = viewer.project;
		if (!viewer.workspaceAdmin) return [];
		const base = `/workspace/${encodeURIComponent(p.workspaceId)}/projects/${encodeURIComponent(p.id)}`;
		return [
			{
				id: 'project',
				labelKey: GROUP_KEYS.project,
				caption: p.name,
				sections: settingsSections(p.typeKey, true, { base }).map((s) => ({
					id: s.id,
					labelKey: s.labelKey,
					href: s.href
				}))
			}
		];
	}

	const groups: SettingsGroupModel[] = [];
	const account: SettingsSectionModel[] = [
		{ id: 'profile', labelKey: 'shell.sections.profile', href: '/settings' },
		{ id: 'plugins', labelKey: 'shell.sections.plugins', href: '/plugins' }
	];
	if (viewer.personalFlow)
		account.push({
			id: 'flow-packages',
			labelKey: 'shell.sections.flowPackages',
			href: '/flow/package',
			external: true
		});
	groups.push({ id: 'account', labelKey: GROUP_KEYS.account, sections: account });

	if (!viewer.workspace) {
		groups.push({
			id: 'workspace',
			labelKey: GROUP_KEYS.workspace,
			chooseWorkspace: true,
			sections: []
		});
	} else if (viewer.workspaceAdmin) {
		const base = `/workspace/${encodeURIComponent(viewer.workspace.id)}`;
		groups.push({
			id: 'workspace',
			labelKey: GROUP_KEYS.workspace,
			caption: viewer.workspace.name,
			sections: [
				{ id: 'general', labelKey: 'shell.sections.general', href: `${base}/settings` },
				{ id: 'members', labelKey: 'shell.sections.members', href: `${base}/members` },
				{ id: 'webhooks', labelKey: 'shell.sections.webhooks', href: `${base}/webhooks` },
				{ id: 'connections', labelKey: 'nav.connections', href: `${base}/connections` },
				{ id: 'workflows', labelKey: 'shell.sections.workflows', href: `${base}/workflows` },
				{ id: 'flow', labelKey: 'shell.sections.flow', href: `${base}/settings/flow` }
			]
		});
	}

	if (viewer.instanceAdmin)
		groups.push({
			id: 'instance',
			labelKey: GROUP_KEYS.instance,
			sections: [
				{ id: 'dashboard', labelKey: 'shell.sections.dashboard', href: '/admin' },
				{ id: 'users', labelKey: 'shell.sections.users', href: '/admin/users' },
				{ id: 'system', labelKey: 'shell.sections.system', href: '/admin/settings' }
			]
		});
	return groups;
}

/**
 * The section a route belongs to, looked up in the viewer's groups. Null outside settings mode,
 * and for routes whose section is not listed (for example the `/admin/ui` gallery).
 */
export function sectionFor(
	ctx: RouteContext,
	groups: readonly SettingsGroupModel[]
): { group: SettingsGroupModel; section: SettingsSectionModel } | null {
	if (!ctx.settings) return null;
	const { group: groupId, section: sectionId } = ctx.settings;
	const group = groups.find((g) => g.id === groupId);
	const section = group?.sections.find((s) => s.id === sectionId);
	return group && section ? { group, section } : null;
}
