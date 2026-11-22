/**
 * Rows a project bot connection made (its bot token and its webhook) are marked `managed_by` by
 * the Bots and Webhooks endpoints. The marker is read at the boundary: anything that is not the
 * documented shape reads as "not managed", so a malformed answer never locks or unlocks a row by
 * accident of typing.
 */

export interface ManagedProject {
	id: string;
	name: string;
}

export interface ManagedBy {
	kind: 'project_connection';
	projects: ManagedProject[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** `managed_by` of a bot or webhook answer, or `null` when the row is not managed. */
export function parseManagedBy(value: unknown): ManagedBy | null {
	if (!isRecord(value) || value.kind !== 'project_connection' || !Array.isArray(value.projects))
		return null;
	const projects: ManagedProject[] = [];
	for (const project of value.projects) {
		if (isRecord(project) && typeof project.id === 'string' && typeof project.name === 'string')
			projects.push({ id: project.id, name: project.name });
	}
	return { kind: 'project_connection', projects };
}

/** The Execution section of a project's settings, where a managed row is changed. */
export function executionHref(workspaceId: string, projectId: string): string {
	return `/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}/settings/execution`;
}

/** The ids of the workspace's bots that a connection manages. */
export function managedBotIds(bots: readonly { id: string; managed_by?: unknown }[]): Set<string> {
	return new Set(
		bots.filter((bot) => parseManagedBy(bot.managed_by) !== null).map((bot) => bot.id)
	);
}

export const MANAGED_REASON_KEY = 'execution.connection.managed.reason';

/**
 * What a managed row offers: Edit, Delete, Rotate secret and Revoke are disabled, with the one
 * reason shown beside them. An unmanaged row is not locked and has no reason.
 */
export function managedLock(managedBy: ManagedBy | null): {
	disabled: boolean;
	reasonKey: string | undefined;
} {
	return managedBy === null
		? { disabled: false, reasonKey: undefined }
		: { disabled: true, reasonKey: MANAGED_REASON_KEY };
}

/** The bots a webhook may name: a managed bot's deliveries belong to its connection. */
export function pickableBots<T extends { user_id: string }>(
	bots: readonly T[],
	managed: ReadonlySet<string>
): T[] {
	return bots.filter((bot) => !managed.has(bot.user_id));
}
