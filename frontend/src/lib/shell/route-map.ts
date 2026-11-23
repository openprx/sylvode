export type Destination = 'home' | 'projects' | 'flow' | 'governance';
export type SettingsGroup = 'account' | 'workspace' | 'instance' | 'project';

export interface RouteContext {
	/** null only in settings mode. */
	destination: Destination | null;
	settings: { group: SettingsGroup; section: string } | null;
	/** From `/workspace/{w}/...`. */
	workspaceId: string | null;
	/** From `.../projects/{p}/...` (or the legacy `/projects/{p}/...`). */
	projectId: string | null;
	/** `?form=` on the forms route. */
	formId: string | null;
	flowObjectId: string | null;
}

const GOVERNANCE_ROOTS = new Set([
	'governance',
	'proposals',
	'proposal-templates',
	'impact-reviews',
	'decisions',
	'trust-board',
	'trust-scores'
]);
const WORKSPACE_SETTINGS_PAGES = new Set(['members', 'webhooks', 'connections', 'workflows']);
const FLOW_NON_OBJECTS = new Set(['package', 'conversions']);
const ADMIN_SECTIONS: Record<string, string> = {
	users: 'users',
	settings: 'system',
	ui: 'gallery'
};

function settings(group: SettingsGroup, section: string): RouteContext['settings'] {
	return { group, section };
}

/**
 * Which shell frame a URL belongs to (destination column or settings mode), and the ids the
 * shell needs from it. First match wins, in the order of the route table; an unknown path under
 * the app group falls back to Home. `routeId` is accepted for callers that have it; the pathname
 * decides.
 */
export function resolveRoute(url: URL, routeId: string | null = null): RouteContext {
	void routeId;
	const segs = url.pathname.split('/').filter(Boolean);
	const inWorkspace = segs[0] === 'workspace' && Boolean(segs[1]);
	const workspaceId = inWorkspace ? segs[1] : null;
	const wsSection = inWorkspace ? segs[2] : undefined;

	let projectId: string | null = null;
	if (inWorkspace && wsSection === 'projects' && segs[3]) projectId = segs[3];
	else if (segs[0] === 'projects' && segs[1]) projectId = segs[1];

	const formId =
		inWorkspace && wsSection === 'projects' && segs[4] === 'forms'
			? url.searchParams.get('form')
			: null;

	let flowObjectId: string | null = null;
	const flowSegs =
		segs[0] === 'flow' ? segs.slice(1) : inWorkspace && wsSection === 'flow' ? segs.slice(3) : null;
	if (flowSegs && flowSegs[0] && !FLOW_NON_OBJECTS.has(flowSegs[0])) {
		if (flowSegs.length === 1 || (flowSegs.length === 2 && flowSegs[1] === 'convert'))
			flowObjectId = flowSegs[0];
	}

	const base = { workspaceId, projectId, formId, flowObjectId };
	const inSettings = (s: RouteContext['settings']): RouteContext => ({
		destination: null,
		settings: s,
		...base
	});
	const inDestination = (destination: Destination): RouteContext => ({
		destination,
		settings: null,
		...base
	});

	if (inWorkspace && wsSection === 'projects' && segs[4] === 'settings')
		return inSettings(settings('project', segs[5] ?? 'plugins'));
	if (inWorkspace && wsSection === 'settings')
		return inSettings(settings('workspace', segs[3] === 'flow' ? 'flow' : 'general'));
	if (inWorkspace && wsSection && WORKSPACE_SETTINGS_PAGES.has(wsSection))
		return inSettings(settings('workspace', wsSection));
	if (segs[0] === 'settings') return inSettings(settings('account', 'profile'));
	if (segs[0] === 'plugins') return inSettings(settings('account', 'plugins'));
	if (segs[0] === 'admin')
		return inSettings(settings('instance', (segs[1] && ADMIN_SECTIONS[segs[1]]) || 'dashboard'));

	if (segs[0] === 'flow' || segs[0] === 'shared-blocks' || (inWorkspace && wsSection === 'flow'))
		return inDestination('flow');
	if ((inWorkspace && wsSection === 'projects') || segs[0] === 'projects')
		return inDestination('projects');
	if ((segs[0] && GOVERNANCE_ROOTS.has(segs[0])) || (segs[0] === 'users' && segs[2] === 'trust'))
		return inDestination('governance');
	return inDestination('home');
}
