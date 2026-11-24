/**
 * Project path builders and the project settings landing redirect target.
 *
 * Pure functions only. The settings landing route reads the project, calls these builders and
 * throws `redirect()`; the frontend is a static single-page build, so every redirect is
 * client-side. The query string and the fragment of the old URL are carried to the new one.
 */

import { settingsLanding } from '#lib/project/nav.js';

/** Temporary redirect: nothing is cached as moved for good. */
export const REDIRECT_STATUS = 307;

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
