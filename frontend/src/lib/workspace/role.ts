// The signed-in user's role in the workspace the sidebar is showing (FP-R1 R2). The sidebar's
// workspace-admin links (members, webhooks, connections, settings, Flow settings) follow the
// same gate the server applies on those endpoints: the caller's MEMBER role must be `owner` or
// `admin` (`routes/project_type.rs` / `policy.rs`). An instance admin who is a plain member or
// not a member at all does not pass that gate, so it does not see the links either.
//
// The role is re-read whenever the workspace in the URL (or the signed-in user) changes, and it
// is `null` -- links hidden -- from the moment of the change until the new read lands. A slow
// read for a workspace the user already left is dropped.

import type { ApiResult, PaginatedData } from '$lib/api/client';
import type { WorkspaceMember } from '$lib/api/workspaces';
import { isWorkspaceAdminRole, type WorkspaceRole } from './home';

export interface SidebarViewer {
	/** The user's member role in the current workspace; `null` when unknown or not a member. */
	readonly workspaceRole: WorkspaceRole | null;
	/** Instance admin flag. Deliberately NOT a grant: the workspace endpoints ignore it. */
	readonly instanceAdmin: boolean;
}

/** Whether the sidebar renders the workspace-admin links for this viewer. */
export function showWorkspaceAdminLinks(viewer: SidebarViewer): boolean {
	return isWorkspaceAdminRole(viewer.workspaceRole);
}

/** The role of `userId` in one members-list result; `null` on any failure or when absent. */
export function roleFromMembersResult(
	result: ApiResult<PaginatedData<WorkspaceMember>> | null,
	userId: string
): WorkspaceRole | null {
	if (!result || result.code !== 0 || !result.data) return null;
	return result.data.items.find((member) => member.user_id === userId)?.role ?? null;
}

export type MembersFetcher = (
	workspaceId: string
) => Promise<ApiResult<PaginatedData<WorkspaceMember>>>;

/**
 * Tracks the role for the (workspace, user) pair currently on screen. `select` reports `null`
 * synchronously (fail closed) before it starts a read, and only the newest read may report its
 * role.
 */
export class WorkspaceRoleTracker {
	private generation = 0;
	private key: string | null = null;

	constructor(
		private readonly fetchMembers: MembersFetcher,
		private readonly onChange: (role: WorkspaceRole | null) => void
	) {}

	async select(workspaceId: string | null, userId: string | null): Promise<WorkspaceRole | null> {
		const key = workspaceId && userId ? `${workspaceId}\u0000${userId}` : null;
		if (key !== null && key === this.key) return null;
		this.key = key;
		const mine = ++this.generation;
		this.onChange(null);
		if (!workspaceId || !userId) return null;
		let role: WorkspaceRole | null = null;
		try {
			role = roleFromMembersResult(await this.fetchMembers(workspaceId), userId);
		} catch {
			role = null;
		}
		if (mine !== this.generation) return null;
		this.onChange(role);
		return role;
	}
}
