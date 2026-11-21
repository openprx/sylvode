// Landing of the project settings: redirects to the section for the project's type.
import { redirect } from '@sveltejs/kit';
import { projectsApi } from '#lib/api/projects.js';
import { REDIRECT_STATUS, projectSettingsTarget } from '#lib/shell/redirects.js';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ params, url }) => {
	const result = await projectsApi.get(params.projectId);
	if (!result.data) return { notFound: true };
	redirect(
		REDIRECT_STATUS,
		projectSettingsTarget(
			params.workspaceId,
			params.projectId,
			result.data.type_key,
			url.search,
			url.hash
		)
	);
};
