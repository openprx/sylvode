import { botsApi } from '#lib/api/bots.js';
import { formsApi } from '#lib/api/forms.js';
import { workspacesApi } from '#lib/api/workspaces.js';
import { FORMS_IN_TREE } from '../../project/nav';
import type { ColumnApis } from './column-data.svelte';

/** The existing API clients the column reads from. */
export const defaultColumnApis: ColumnApis = {
	workspaces: () => workspacesApi.list(),
	bots: (workspaceId) => botsApi.list(workspaceId),
	forms: (projectId) => formsApi.list(projectId, { per_page: FORMS_IN_TREE })
};
