// Workspace home (`/workspace/{id}`, task FP-N6): the pure decisions behind the page -- which
// cards render for which viewer, which projects make the "recent" list, how the document title
// is built -- plus the loader that reads the workspace, its members and its projects. The page
// component keeps only ephemeral UI state, so everything here runs under bun without a browser.

import { writable } from 'svelte/store';
import { authApi } from '$lib/api/auth';
import type { ApiResult, PaginatedData } from '$lib/api/client';
import { projectsApi, type Project } from '$lib/api/projects';
import { workspacesApi, type Workspace, type WorkspaceMember } from '$lib/api/workspaces';

export type WorkspaceRole = WorkspaceMember['role'];

/** How the Sylvode Flow card renders: a link into Flow, a link to enable it, or not at all. */
export type FlowCardMode = 'open' | 'enable' | 'hidden';

export interface WorkspaceHomeCards {
	readonly flow: FlowCardMode;
	readonly admin: boolean;
}

/** Number of projects listed on the Projects card. */
export const RECENT_PROJECT_LIMIT = 5;

/** The workspace-admin roles: the same `owner`/`admin` gate the server applies on every
 * workspace-admin endpoint (`require_flow_workspace_admin_access`, webhooks, members). */
export function isWorkspaceAdminRole(role: WorkspaceRole | null | undefined): boolean {
	return role === 'owner' || role === 'admin';
}

/**
 * Card visibility for one viewer. `flowEnabled` must be the fail-closed value from
 * `flowFeatureStore` (`true` only after a successful read that said so).
 *
 * | flowEnabled | admin | Flow card | admin card |
 * |---|---|---|---|
 * | true  | yes | open   | shown  |
 * | true  | no  | open   | hidden |
 * | false | yes | enable | shown  |
 * | false | no  | hidden | hidden |
 */
export function workspaceHomeCards(flowEnabled: boolean, isAdmin: boolean): WorkspaceHomeCards {
	let flow: FlowCardMode = 'hidden';
	if (flowEnabled === true) flow = 'open';
	else if (isAdmin === true) flow = 'enable';
	return { flow, admin: isAdmin === true };
}

export type AdminLinkKey = 'members' | 'webhooks' | 'connections' | 'settings';

/** SvelteKit route ids of the admin card's targets; the page resolves them with `resolve()`. */
export type AdminLinkRoute =
	| '/(app)/workspace/[workspaceId]/members'
	| '/(app)/workspace/[workspaceId]/webhooks'
	| '/(app)/workspace/[workspaceId]/connections'
	| '/(app)/workspace/[workspaceId]/settings';

export interface AdminLink {
	readonly key: AdminLinkKey;
	readonly route: AdminLinkRoute;
	readonly labelKey: string;
}

/** The admin card's four links, in display order. Rendered only when `cards.admin` is true. */
export const WORKSPACE_ADMIN_LINKS: readonly AdminLink[] = [
	{ key: 'members', route: '/(app)/workspace/[workspaceId]/members', labelKey: 'nav.members' },
	{ key: 'webhooks', route: '/(app)/workspace/[workspaceId]/webhooks', labelKey: 'nav.webhook' },
	{
		key: 'connections',
		route: '/(app)/workspace/[workspaceId]/connections',
		labelKey: 'nav.connections'
	},
	{
		key: 'settings',
		route: '/(app)/workspace/[workspaceId]/settings',
		labelKey: 'nav.workspaceSettings'
	}
];

function timestamp(value: string | undefined): number {
	const parsed = value ? Date.parse(value) : Number.NaN;
	return Number.isNaN(parsed) ? 0 : parsed;
}

/** The most recently updated projects (ties broken by creation time), at most `limit`. Does not
 * mutate the input. */
export function recentProjects(
	projects: readonly Project[],
	limit: number = RECENT_PROJECT_LIMIT
): Project[] {
	return [...projects]
		.sort(
			(a, b) =>
				timestamp(b.updated_at) - timestamp(a.updated_at) ||
				timestamp(b.created_at) - timestamp(a.created_at)
		)
		.slice(0, Math.max(0, limit));
}

/** `/workspace/{id}` exactly (an optional trailing slash included), never a sub-page. */
export function isWorkspaceHomePath(pathname: string): boolean {
	return /^\/workspace\/[^/]+\/?$/.test(pathname);
}

export type Translate = (key: string, options?: { values?: Record<string, string> }) => string;

/** `Sylvode - <workspace name>` once the name is known, `pageTitle.workspaceHome` before. */
export function workspaceHomeTitle(translate: Translate, name: string | null | undefined): string {
	const trimmed = name?.trim();
	if (!trimmed) return translate('pageTitle.workspaceHome');
	return translate('pageTitle.workspaceHomeWithName', { values: { name: trimmed } });
}

/** Workspace names the home page has loaded, keyed by id; the app layout reads it for the
 * document title so the title follows the page without a second workspace request. */
export const workspaceHomeNames = writable<Record<string, string>>({});

export function rememberWorkspaceName(workspaceId: string, name: string): void {
	workspaceHomeNames.update((current) => ({ ...current, [workspaceId]: name }));
}

export interface WorkspaceHomeDeps {
	readonly getWorkspace: (workspaceId: string) => Promise<ApiResult<Workspace>>;
	readonly getMembers: (workspaceId: string) => Promise<ApiResult<PaginatedData<WorkspaceMember>>>;
	readonly listProjects: (workspaceId: string) => Promise<ApiResult<PaginatedData<Project>>>;
	readonly currentUserId: () => Promise<string | null>;
}

/** The real API calls: `GET /workspaces/{id}`, `GET /workspaces/{id}/members`,
 * `GET /workspaces/{id}/projects`, `GET /auth/me`. */
export const defaultWorkspaceHomeDeps: WorkspaceHomeDeps = {
	getWorkspace: (workspaceId) => workspacesApi.get(workspaceId),
	getMembers: (workspaceId) => workspacesApi.getMembers(workspaceId),
	listProjects: (workspaceId) => projectsApi.list(workspaceId),
	currentUserId: async () => {
		const me = await authApi.me();
		return me.code === 0 ? (me.data?.user.id ?? null) : null;
	}
};

export type WorkspaceHomeLoad =
	| {
			readonly status: 'ready';
			readonly workspace: Workspace;
			readonly role: WorkspaceRole | null;
			readonly projectTotal: number;
			readonly recent: readonly Project[];
			readonly projectsFailed: boolean;
	  }
	| { readonly status: 'not_found' }
	| { readonly status: 'failed' };

async function settle<T>(promise: Promise<T>): Promise<T | null> {
	try {
		return await promise;
	} catch {
		return null;
	}
}

/**
 * Reads the workspace, the viewer's role and the project list in parallel. A workspace the
 * viewer cannot read (403/404, legacy envelope or typed) is `not_found`, so the page does not
 * reveal whether it exists; any other failure is `failed`. The role fails closed to `null`
 * (no admin card, no Flow enable link). Branches on numeric `code` / `error_code` only.
 */
export async function loadWorkspaceHome(
	workspaceId: string,
	deps: WorkspaceHomeDeps
): Promise<WorkspaceHomeLoad> {
	const [workspace, members, projects, userId] = await Promise.all([
		settle(deps.getWorkspace(workspaceId)),
		settle(deps.getMembers(workspaceId)),
		settle(deps.listProjects(workspaceId)),
		settle(deps.currentUserId())
	]);

	if (!workspace || workspace.code !== 0 || !workspace.data) {
		const code = workspace?.code;
		const errorCode = workspace?.error_code;
		if (code === 403 || code === 404 || errorCode === 'forbidden' || errorCode === 'not_found') {
			return { status: 'not_found' };
		}
		return { status: 'failed' };
	}

	let role: WorkspaceRole | null = null;
	if (userId && members && members.code === 0 && members.data) {
		role = members.data.items.find((member) => member.user_id === userId)?.role ?? null;
	}

	const projectsOk = !!projects && projects.code === 0 && !!projects.data;
	const items = projectsOk ? (projects.data?.items ?? []) : [];
	const total = projectsOk ? Math.max(projects.data?.total ?? 0, items.length) : 0;

	return {
		status: 'ready',
		workspace: workspace.data,
		role,
		projectTotal: total,
		recent: recentProjects(items),
		projectsFailed: !projectsOk
	};
}
