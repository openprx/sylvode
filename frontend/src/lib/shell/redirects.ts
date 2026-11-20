/**
 * Redirect targets of the Phase 3 route moves: the single source the old-location stubs import.
 *
 * Pure functions only. The stubs (`routes/(app)/projects/[pid]/**` and the project settings
 * landing) read the project, call these builders and throw `redirect()`; the frontend is a static
 * single-page build, so every redirect is client-side. The query string and the fragment of the
 * old URL are carried to the new one.
 */

import { settingsLanding } from '#lib/project/nav.js';

/** Temporary redirect: the old URLs stay valid, nothing is cached as moved for good. */
export const REDIRECT_STATUS = 307;

/** Pages that moved from `/projects/{pid}/…` to `/workspace/{w}/projects/{pid}/…`. */
export const MOVED_PROJECT_PAGES = ['ai-agents', 'audit-reports'] as const;
export type MovedProjectPage = (typeof MOVED_PROJECT_PAGES)[number];

function withTail(path: string, search: string, hash: string): string {
	const query = search && search !== '?' ? (search.startsWith('?') ? search : `?${search}`) : '';
	const fragment = hash && hash !== '#' ? (hash.startsWith('#') ? hash : `#${hash}`) : '';
	return `${path}${query}${fragment}`;
}

/** `/workspace/{w}/projects/{p}` plus an optional rest path, query and fragment. */
export function projectPath(
	workspaceId: string,
	projectId: string,
	rest = '',
	search = '',
	hash = ''
): string {
	const tail = rest ? `/${rest.replace(/^\/+/, '')}` : '';
	return withTail(
		`/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}${tail}`,
		search,
		hash
	);
}

export interface LegacyProjectUrl {
	projectId: string;
	/** Path after the project id, without a leading slash ("audit-reports/7"). */
	rest: string;
}

/**
 * Reads `/projects/{pid}/ai-agents`, `/projects/{pid}/audit-reports` and
 * `/projects/{pid}/audit-reports/{id}`. Anything else (including a trailing slash variant of an
 * unknown page, or a path that is already under a workspace) is `null` and stays untouched.
 */
export function parseLegacyProjectUrl(pathname: string): LegacyProjectUrl | null {
	const match = /^\/projects\/([^/]+)\/(ai-agents|audit-reports(?:\/[^/]+)?)\/?$/.exec(pathname);
	if (!match) return null;
	let projectId: string;
	try {
		projectId = decodeURIComponent(match[1]);
	} catch {
		return null;
	}
	return { projectId, rest: match[2] };
}

/** The new location of a legacy project URL once the project's workspace is known. */
export function legacyProjectTarget(
	pathname: string,
	workspaceId: string,
	search = '',
	hash = ''
): string | null {
	const legacy = parseLegacyProjectUrl(pathname);
	if (!legacy) return null;
	return projectPath(workspaceId, legacy.projectId, legacy.rest, search, hash);
}

/** `/workspace/{w}/projects/{p}/settings` → its landing section. */
export function projectSettingsTarget(
	workspaceId: string,
	projectId: string,
	typeKey: string | null | undefined,
	search = '',
	hash = ''
): string {
	return withTail(settingsLanding(workspaceId, projectId, typeKey ?? ''), search, hash);
}
