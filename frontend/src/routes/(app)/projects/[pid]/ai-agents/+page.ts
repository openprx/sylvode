// Old location of a page that moved under the workspace project path; see `lib/shell/redirects.ts`.
import { redirect } from '@sveltejs/kit';
import { projectsApi } from '#lib/api/projects.js';
import { REDIRECT_STATUS, legacyProjectTarget } from '#lib/shell/redirects.js';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ url }) => {
	const projectId = url.pathname.split('/')[2] ?? '';
	const result = await projectsApi.get(decodeURIComponent(projectId));
	const target = result.data
		? legacyProjectTarget(url.pathname, result.data.workspace_id, url.search, url.hash)
		: null;
	if (target) redirect(REDIRECT_STATUS, target);
	return { notFound: true };
};
