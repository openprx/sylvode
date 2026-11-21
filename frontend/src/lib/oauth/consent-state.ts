/**
 * What the consent page lets the person do, as a pure function of the data the server sent and
 * what they have chosen so far (contract 6.1 step 7 and 14.1). The page renders this and nothing
 * else decides whether Allow is enabled, so the rules are tested without a DOM.
 */

export type ProjectMode = 'all' | 'selected';
export type ProjectsStatus = 'idle' | 'loading' | 'ready' | 'failed';

export interface WorkspaceChoice {
	id: string;
	name: string;
	eligible: boolean;
	reason: string | null;
}

export interface ScopeChoice {
	name: string;
	required: boolean;
}

export interface ConsentSelection {
	workspaceId: string | null;
	projectMode: ProjectMode | null;
	projectIds: readonly string[];
	/** The optional scopes the person ticked. Required ones are always included. */
	optionalScopes: ReadonlySet<string>;
}

export interface ConsentInput {
	workspaces: readonly WorkspaceChoice[];
	scopes: readonly ScopeChoice[];
	/** Resource names of the request: `api` and `mcp`. */
	resources: readonly string[];
	selection: ConsentSelection;
	/** False once the server said the application can no longer be connected (disabled). */
	clientAvailable: boolean;
	/** State of the project list of the chosen workspace. */
	projectsStatus: ProjectsStatus;
}

/** Why Allow is disabled; the page maps each to one sentence shown next to the button. */
export type AllowBlock =
	| 'client_unavailable'
	| 'no_eligible_workspace'
	| 'choose_workspace'
	| 'choose_projects'
	| 'choose_selected_projects'
	| 'projects_loading'
	| 'projects_failed'
	| 'no_scopes'
	| 'scopes_miss_resource';

export interface ConsentView {
	eligibleWorkspaceIds: string[];
	/** No membership allows connected applications: only the way back is offered. */
	noEligibleWorkspace: boolean;
	/** The chosen workspace, only while it is still eligible. */
	chosenWorkspace: WorkspaceChoice | null;
	/** Scopes the decision would grant: every required one plus the ticked optional ones. */
	chosenScopes: string[];
	/** Null when Allow is enabled. */
	allowBlock: AllowBlock | null;
	allowEnabled: boolean;
	/** Selecting projects is possible: a workspace is chosen and the mode is `selected`. */
	showProjectPicker: boolean;
}

/** Scopes each resource supports (contract 7.2); `offline_access` is a grant property only. */
const MCP_SCOPES: ReadonlySet<string> = new Set([
	'projects:read',
	'work_items:read',
	'work_items:write',
	'forms:read',
	'forms:write'
]);
const API_EXCLUDED: ReadonlySet<string> = new Set(['offline_access']);

function supports(resource: string, scope: string): boolean {
	if (resource === 'mcp') return MCP_SCOPES.has(scope);
	if (resource === 'api') return !API_EXCLUDED.has(scope);
	return true;
}

export function evaluateConsent(input: ConsentInput): ConsentView {
	const { selection } = input;
	const eligible = input.workspaces.filter((workspace) => workspace.eligible);
	const eligibleWorkspaceIds = eligible.map((workspace) => workspace.id);
	const chosenWorkspace =
		eligible.find((workspace) => workspace.id === selection.workspaceId) ?? null;

	const requiredNames = input.scopes.filter((scope) => scope.required).map((scope) => scope.name);
	const optionalNames = input.scopes.filter((scope) => !scope.required).map((scope) => scope.name);
	const chosenScopes = [
		...requiredNames,
		...optionalNames.filter((name) => selection.optionalScopes.has(name))
	];

	let allowBlock: AllowBlock | null = null;
	if (!input.clientAvailable) allowBlock = 'client_unavailable';
	else if (eligible.length === 0) allowBlock = 'no_eligible_workspace';
	else if (chosenWorkspace === null) allowBlock = 'choose_workspace';
	else if (selection.projectMode === null) allowBlock = 'choose_projects';
	else if (selection.projectMode === 'selected' && input.projectsStatus === 'loading')
		allowBlock = 'projects_loading';
	else if (selection.projectMode === 'selected' && input.projectsStatus === 'failed')
		allowBlock = 'projects_failed';
	else if (selection.projectMode === 'selected' && selection.projectIds.length === 0)
		allowBlock = 'choose_selected_projects';
	else if (chosenScopes.length === 0) allowBlock = 'no_scopes';
	else if (
		input.resources.some((resource) => !chosenScopes.some((scope) => supports(resource, scope)))
	)
		allowBlock = 'scopes_miss_resource';

	return {
		eligibleWorkspaceIds,
		noEligibleWorkspace: eligible.length === 0,
		chosenWorkspace,
		chosenScopes,
		allowBlock,
		allowEnabled: allowBlock === null,
		showProjectPicker: chosenWorkspace !== null && selection.projectMode === 'selected'
	};
}

/** The selection after the person picks a workspace: the project choice belongs to a workspace. */
export function selectWorkspace(
	selection: ConsentSelection,
	workspaceId: string
): ConsentSelection {
	if (selection.workspaceId === workspaceId) return selection;
	return { ...selection, workspaceId, projectIds: [] };
}

/** The initial selection: the preselected workspace only when it is eligible; nothing else. */
export function initialSelection(
	workspaces: readonly WorkspaceChoice[],
	preselectedWorkspaceId: string | null
): ConsentSelection {
	const preselected = workspaces.find(
		(workspace) => workspace.eligible && workspace.id === preselectedWorkspaceId
	);
	return {
		workspaceId: preselected?.id ?? null,
		projectMode: null,
		projectIds: [],
		optionalScopes: new Set()
	};
}

/** The workspaces with one marked as no longer eligible (the server refused it). */
export function markIneligible(
	workspaces: readonly WorkspaceChoice[],
	workspaceId: string
): WorkspaceChoice[] {
	return workspaces.map((workspace) =>
		workspace.id === workspaceId
			? { ...workspace, eligible: false, reason: workspace.reason ?? 'workspace_disallows' }
			: workspace
	);
}

export interface AllowBody {
	decision: 'allow';
	workspace_id: string;
	project_mode: ProjectMode;
	project_ids?: string[];
	scopes: string[];
}

/** The body of the allow decision; null while the view does not allow it. */
export function allowBody(input: ConsentInput): AllowBody | null {
	const view = evaluateConsent(input);
	const { selection } = input;
	if (!view.allowEnabled || view.chosenWorkspace === null || selection.projectMode === null)
		return null;
	return selection.projectMode === 'all'
		? {
				decision: 'allow',
				workspace_id: view.chosenWorkspace.id,
				project_mode: 'all',
				scopes: view.chosenScopes
			}
		: {
				decision: 'allow',
				workspace_id: view.chosenWorkspace.id,
				project_mode: 'selected',
				project_ids: [...selection.projectIds],
				scopes: view.chosenScopes
			};
}

/**
 * The only addresses the page ever navigates to are the `redirect_to` values the server returns.
 * This guards the navigation itself: an absolute http or https URL without credentials. The
 * server has already matched it to a registered return address.
 */
export function isNavigableRedirect(value: string): boolean {
	if (value.length === 0 || value.length > 4096) return false;
	// eslint-disable-next-line no-control-regex
	if (/[\u0000-\u001f\u007f\s\\]/.test(value)) return false;
	let url: URL;
	try {
		url = new URL(value);
	} catch {
		return false;
	}
	return (url.protocol === 'http:' || url.protocol === 'https:') && !url.username && !url.password;
}
