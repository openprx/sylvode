// What the "Project access" dialog of a workspace bot token shows and sends. The server enforces the
// same rules (a workspace-wide token carries no project level; a comment or write level also needs a
// token with `write`); these helpers keep the screen from offering what the server would refuse.

import type { Bot, BotProjectAccess } from '#lib/api/bots.js';
import type { ApiResult } from '#lib/api/client.js';
import { plural } from '#lib/ui/foundations/plural.js';

export type ProjectScope = 'workspace' | 'projects';
export type DefaultLevel = 'none' | 'read' | 'comment';
export type GrantLevel = 'read' | 'comment' | 'write';

export const DEFAULT_LEVELS: readonly DefaultLevel[] = ['none', 'read', 'comment'];
export const GRANT_LEVELS: readonly GrantLevel[] = ['read', 'comment', 'write'];

export const MANAGED_KEY = 'botAccess.managed';

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (
	key: string,
	options?: { values?: Record<string, unknown>; default?: string }
) => string;

export function isDefaultLevel(value: unknown): value is DefaultLevel {
	return DEFAULT_LEVELS.includes(value as DefaultLevel);
}

export function isGrantLevel(value: unknown): value is GrantLevel {
	return GRANT_LEVELS.includes(value as GrantLevel);
}

export interface AccessForm {
	scope: ProjectScope;
	/** Level in every project without a grant; `none` while the scope is `workspace`. */
	defaultLevel: DefaultLevel;
	/** Project id -> level. A project without an entry has the default level. */
	grants: Record<string, GrantLevel>;
}

export const WORKSPACE_FORM: AccessForm = { scope: 'workspace', defaultLevel: 'none', grants: {} };

/** The form for what the server answered. An unknown scope or level reads as the narrowest meaning. */
export function formFromAccess(access: BotProjectAccess): AccessForm {
	if (access.project_scope !== 'projects') return { ...WORKSPACE_FORM, grants: {} };
	const grants: Record<string, GrantLevel> = {};
	for (const grant of access.grants ?? []) {
		if (typeof grant?.project_id === 'string' && isGrantLevel(grant.level))
			grants[grant.project_id] = grant.level;
	}
	return {
		scope: 'projects',
		defaultLevel: isDefaultLevel(access.default_level) ? access.default_level : 'none',
		grants
	};
}

/**
 * The body of a save. A workspace-wide token sends no level and no grant (the server refuses
 * both). Grants are listed in `order` first (the projects as shown), then any other grant the form
 * still carries, so a grant on a project that is not in the loaded list is not dropped silently.
 */
export function accessRequest(form: AccessForm, order: readonly string[] = []): BotProjectAccess {
	if (form.scope === 'workspace')
		return { project_scope: 'workspace', default_level: 'none', grants: [] };
	const ids = [...order.filter((id) => id in form.grants)];
	for (const id of Object.keys(form.grants)) if (!ids.includes(id)) ids.push(id);
	return {
		project_scope: 'projects',
		default_level: form.defaultLevel,
		grants: ids.map((id) => ({ project_id: id, level: form.grants[id] }))
	};
}

/** Sets or clears (`''`) the level of one project; the other grants stay. */
export function withGrant(form: AccessForm, projectId: string, level: GrantLevel | ''): AccessForm {
	const grants = { ...form.grants };
	if (level === '') delete grants[projectId];
	else grants[projectId] = level;
	return { ...form, grants };
}

/** A token is workspace-wide until the owner picks projects; picking "All" clears the levels. */
export function withScope(form: AccessForm, scope: ProjectScope): AccessForm {
	return scope === 'workspace' ? { ...WORKSPACE_FORM, grants: {} } : { ...form, scope };
}

/** The level a project has under this form: its grant, else the default. */
export function levelFor(form: AccessForm, projectId: string): GrantLevel | DefaultLevel {
	return form.grants[projectId] ?? form.defaultLevel;
}

/**
 * True when the form gives a comment or write level but the token cannot write: such a level only
 * takes effect with `write` (or `admin`) among the token permissions.
 */
export function needsWritePermission(form: AccessForm, permissions: readonly string[]): boolean {
	if (form.scope !== 'projects') return false;
	if (permissions.includes('write') || permissions.includes('admin')) return false;
	return (
		form.defaultLevel === 'comment' ||
		Object.values(form.grants).some((level) => level === 'comment' || level === 'write')
	);
}

export type AccessOutcome =
	| { kind: 'managed' }
	| { kind: 'forbidden' }
	| { kind: 'failed'; result: ApiResult<unknown> };

/** What a refused read or save means on screen: 409 is a bot a project connection manages. */
export function accessOutcome(result: ApiResult<unknown>): AccessOutcome {
	if (result.code === 409) return { kind: 'managed' };
	if (result.code === 403) return { kind: 'forbidden' };
	return { kind: 'failed', result };
}

/** The list column of a token: its reach in words. A bot of a project connection says so instead. */
export function accessSummary(
	bot: Pick<Bot, 'project_scope' | 'default_project_level' | 'grant_count'>,
	managed: boolean,
	translate: Translate,
	locale: string | null | undefined
): string {
	if (managed) return translate(MANAGED_KEY);
	if (bot.project_scope !== 'projects') return translate('botAccess.summary.workspace');
	const level = isDefaultLevel(bot.default_project_level) ? bot.default_project_level : 'none';
	return translate('botAccess.summary.projects', {
		values: {
			count: plural(translate, 'botAccess.summary.grants', bot.grant_count ?? 0, locale),
			level: translate(`botAccess.defaultLevel.${level}`)
		}
	});
}
