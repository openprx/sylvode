/**
 * The consent page's state logic (`src/lib/oauth/consent-state.ts`): when Allow is enabled and why
 * not, what the decision body holds, and which addresses the page may navigate to.
 *
 * Run standalone: `bun tests/oauth-consent-state.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	allowBody,
	evaluateConsent,
	initialSelection,
	isNavigableRedirect,
	markIneligible,
	selectWorkspace,
	type ConsentInput,
	type ConsentSelection
} from '../src/lib/oauth/consent-state';

const suite = new Suite('oauth-consent-state');

const W1 = { id: 'w1', name: 'Acme', eligible: true, reason: null };
const W2 = { id: 'w2', name: 'Lab', eligible: true, reason: null };
const OFF = { id: 'w3', name: 'Off', eligible: false, reason: 'workspace_disallows' };

const REFERENCE_SCOPES = [
	{ name: 'projects:read', required: true },
	{ name: 'work_items:read', required: true },
	{ name: 'notifications:read', required: true },
	{ name: 'tasks:read', required: true },
	{ name: 'offline_access', required: true }
];

function select(partial: Partial<ConsentSelection> = {}): ConsentSelection {
	return {
		workspaceId: 'w1',
		projectMode: 'all',
		projectIds: [],
		optionalScopes: new Set(),
		...partial
	};
}

function input(partial: Partial<ConsentInput> = {}): ConsentInput {
	return {
		workspaces: [W1, W2],
		scopes: REFERENCE_SCOPES,
		resources: ['api', 'mcp'],
		selection: select(),
		clientAvailable: true,
		projectsStatus: 'idle',
		...partial
	};
}

suite.check('all required scopes, one workspace and all projects: Allow is enabled', () => {
	const view = evaluateConsent(input());
	assertEqual(view.allowEnabled, true, 'enabled');
	assertEqual(view.allowBlock, null, 'no block');
	assertDeepEqual(
		view.chosenScopes,
		REFERENCE_SCOPES.map((scope) => scope.name),
		'scopes'
	);
});

suite.check('no workspace is eligible: only the way back, Allow blocked', () => {
	const view = evaluateConsent(
		input({ workspaces: [OFF], selection: select({ workspaceId: null }) })
	);
	assertEqual(view.noEligibleWorkspace, true, 'flag');
	assertEqual(view.allowBlock, 'no_eligible_workspace', 'block');
	assertEqual(view.allowEnabled, false, 'disabled');
	assertEqual(
		evaluateConsent(input({ workspaces: [] })).noEligibleWorkspace,
		true,
		'no memberships'
	);
});

suite.check('a workspace with connected applications switched off cannot be the chosen one', () => {
	const view = evaluateConsent(
		input({ workspaces: [W1, OFF], selection: select({ workspaceId: 'w3' }) })
	);
	assertEqual(view.chosenWorkspace, null, 'not chosen');
	assertEqual(view.allowBlock, 'choose_workspace', 'block');
	assertDeepEqual(view.eligibleWorkspaceIds, ['w1'], 'eligible list');
});

suite.check('no workspace chosen yet', () => {
	const view = evaluateConsent(input({ selection: select({ workspaceId: null }) }));
	assertEqual(view.allowBlock, 'choose_workspace', 'block');
});

suite.check('no project choice yet', () => {
	const view = evaluateConsent(input({ selection: select({ projectMode: null }) }));
	assertEqual(view.allowBlock, 'choose_projects', 'block');
	assertEqual(view.showProjectPicker, false, 'no picker');
});

suite.check('mode selected with no project chosen blocks Allow and shows the picker', () => {
	const view = evaluateConsent(
		input({ selection: select({ projectMode: 'selected' }), projectsStatus: 'ready' })
	);
	assertEqual(view.allowBlock, 'choose_selected_projects', 'block');
	assertEqual(view.showProjectPicker, true, 'picker');
});

suite.check('mode selected with projects chosen enables Allow', () => {
	const view = evaluateConsent(
		input({
			selection: select({ projectMode: 'selected', projectIds: ['p1', 'p2'] }),
			projectsStatus: 'ready'
		})
	);
	assertEqual(view.allowEnabled, true, 'enabled');
});

suite.check('the project list loading or failing blocks the selected mode', () => {
	const base = select({ projectMode: 'selected', projectIds: ['p1'] });
	assertEqual(
		evaluateConsent(input({ selection: base, projectsStatus: 'loading' })).allowBlock,
		'projects_loading',
		'loading'
	);
	assertEqual(
		evaluateConsent(input({ selection: base, projectsStatus: 'failed' })).allowBlock,
		'projects_failed',
		'failed'
	);
	// All projects does not need the list.
	assertEqual(
		evaluateConsent(input({ selection: select(), projectsStatus: 'failed' })).allowEnabled,
		true,
		'all projects'
	);
});

suite.check('the application was disabled: Allow is blocked whatever else is chosen', () => {
	const view = evaluateConsent(input({ clientAvailable: false }));
	assertEqual(view.allowBlock, 'client_unavailable', 'block');
});

suite.check('required scopes are always included; optional ones only when ticked', () => {
	const scopes = [...REFERENCE_SCOPES, { name: 'work_items:write', required: false }];
	assertDeepEqual(
		evaluateConsent(input({ scopes })).chosenScopes.includes('work_items:write'),
		false,
		'unticked'
	);
	assertDeepEqual(
		evaluateConsent(
			input({ scopes, selection: select({ optionalScopes: new Set(['work_items:write']) }) })
		).chosenScopes.includes('work_items:write'),
		true,
		'ticked'
	);
	// A ticked name the request did not offer is ignored.
	assertDeepEqual(
		evaluateConsent(
			input({ selection: select({ optionalScopes: new Set(['forms:write']) }) })
		).chosenScopes.includes('forms:write'),
		false,
		'not offered'
	);
});

suite.check('only optional scopes and none ticked: no scopes, Allow blocked', () => {
	const scopes = [
		{ name: 'work_items:write', required: false },
		{ name: 'notifications:write', required: false }
	];
	const none = evaluateConsent(input({ scopes }));
	assertEqual(none.allowBlock, 'no_scopes', 'none ticked');
	const some = evaluateConsent(
		input({ scopes, selection: select({ optionalScopes: new Set(['work_items:write']) }) })
	);
	assertEqual(some.allowEnabled, true, 'one scope ticked');
});

suite.check('a requested resource the chosen scopes do not reach blocks Allow', () => {
	// notifications:write is an API scope only; the MCP resource would be left without any scope.
	const scopes = [{ name: 'notifications:write', required: false }];
	const view = evaluateConsent(
		input({ scopes, selection: select({ optionalScopes: new Set(['notifications:write']) }) })
	);
	assertEqual(view.allowBlock, 'scopes_miss_resource', 'mcp not covered');
	const apiOnly = evaluateConsent(
		input({
			scopes,
			resources: ['api'],
			selection: select({ optionalScopes: new Set(['notifications:write']) })
		})
	);
	assertEqual(apiOnly.allowEnabled, true, 'api only request');
	// offline_access alone covers neither resource.
	const offline = evaluateConsent(input({ scopes: [{ name: 'offline_access', required: true }] }));
	assertEqual(offline.allowBlock, 'scopes_miss_resource', 'offline only');
});

suite.check('changing the workspace clears the project choice', () => {
	const before = select({ projectMode: 'selected', projectIds: ['p1'] });
	const after = selectWorkspace(before, 'w2');
	assertEqual(after.workspaceId, 'w2', 'workspace');
	assertDeepEqual(after.projectIds, [], 'projects cleared');
	assertEqual(after.projectMode, 'selected', 'mode kept');
	assertEqual(selectWorkspace(before, 'w1'), before, 'same workspace keeps the selection');
});

suite.check('the initial selection preselects only an eligible workspace and nothing else', () => {
	assertEqual(initialSelection([W1, W2], 'w2').workspaceId, 'w2', 'hint');
	assertEqual(initialSelection([W1, OFF], 'w3').workspaceId, null, 'ineligible hint');
	assertEqual(initialSelection([W1, W2], null).workspaceId, null, 'none');
	const initial = initialSelection([W1], 'w1');
	assertEqual(initial.projectMode, null, 'no project mode');
	assertDeepEqual(initial.projectIds, [], 'no projects');
	assertEqual(initial.optionalScopes.size, 0, 'no optional scopes');
});

suite.check('a workspace the server refused is marked ineligible', () => {
	const marked = markIneligible([W1, W2], 'w1');
	assertEqual(marked[0]?.eligible, false, 'w1');
	assertEqual(marked[1]?.eligible, true, 'w2');
});

suite.check('the allow body carries the choice and nothing the page was not shown', () => {
	assertDeepEqual(
		allowBody(input()),
		{
			decision: 'allow',
			workspace_id: 'w1',
			project_mode: 'all',
			scopes: REFERENCE_SCOPES.map((scope) => scope.name)
		},
		'all projects'
	);
	assertDeepEqual(
		allowBody(
			input({
				selection: select({ projectMode: 'selected', projectIds: ['p1'] }),
				projectsStatus: 'ready'
			})
		),
		{
			decision: 'allow',
			workspace_id: 'w1',
			project_mode: 'selected',
			project_ids: ['p1'],
			scopes: REFERENCE_SCOPES.map((scope) => scope.name)
		},
		'selected'
	);
	assertEqual(allowBody(input({ clientAvailable: false })), null, 'blocked');
	assertEqual(
		allowBody(input({ selection: select({ projectMode: 'selected' }), projectsStatus: 'ready' })),
		null,
		'selected without projects'
	);
});

suite.check('only absolute http(s) redirect addresses without credentials are navigable', () => {
	assert(
		isNavigableRedirect('http://127.0.0.1:5555/oauth/sylvode/callback?code=svc_x&state=y'),
		'loopback'
	);
	assert(isNavigableRedirect('https://app.example/cb?code=1'), 'https');
	for (const bad of [
		'',
		'javascript:alert(1)',
		'data:text/html,x',
		'//evil.example/cb',
		'/relative',
		'http://user:pass@127.0.0.1/cb',
		'http://127.0.0.1/cb\nSet-Cookie: x',
		'http://127.0.0.1/c b',
		'http://127.0.0.1\\@evil.example/',
		'file:///etc/passwd',
		`http://127.0.0.1/${'a'.repeat(5000)}`
	]) {
		assert(!isNavigableRedirect(bad), `refused: ${JSON.stringify(bad.slice(0, 40))}`);
	}
});

finish(suite.result());
