import { flowApi } from '../src/lib/api/flow';
import {
	DEFAULT_FLOW_LIMITS,
	negotiateFlowLimitsVersion,
	wireFieldName
} from '../src/lib/flow/limits';
import { Suite, assert, assertEqual as equal, finish } from './support/harness';
function assertEqual<T>(actual: T, expected: T, message = 'contract value') {
	equal(actual, expected, message);
}
const suite = new Suite('flow_scope_api');
const calls: Array<{ url: string; method: string; body: Record<string, unknown> }> = [];
const originalFetch = globalThis.fetch;
globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
	calls.push({
		url: String(url),
		method: init?.method ?? 'GET',
		body: init?.body ? JSON.parse(String(init.body)) : {}
	});
	return new Response(JSON.stringify({ code: 0, message: 'ok', data: {} }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;
await suite.checkAsync(
	'personal home and capabilities use global endpoints without workspace selection',
	async () => {
		await flowApi.getCapabilities();
		await flowApi.getHome();
		assertEqual(calls[0].url, '/api/v2/flow/capabilities');
		assertEqual(calls[1].url, '/api/v2/flow/home');
	}
);
await suite.checkAsync(
	'creation and typed commands preserve scope/object identity without fabricated workspace',
	async () => {
		await flowApi.createObject('personal-scope', {
			object_type: 'page',
			title: 'Independent',
			idempotency_key: 'create-1'
		});
		const create = calls.at(-1)!;
		assertEqual(create.url, '/api/v2/flow/objects');
		assertEqual(create.body.scope_id, 'personal-scope');
		assert(!('workspace_id' in create.body), 'personal creation does not include a workspace');
		await flowApi.executeCommand('collection-id', {
			command: {
				type: 'record_patch',
				payload: { record_id: 'record-1', values: { note: 'Typed' } }
			},
			idempotency_key: 'command-1'
		});
		assertEqual(calls.at(-1)!.url, '/api/v2/flow/objects/collection-id/commands');
	}
);
await suite.checkAsync(
	'scope navigation and search cannot fall back to a selected workspace',
	async () => {
		await flowApi.getNavigator('scope-1');
		assertEqual(calls.at(-1)!.url, '/api/v2/flow/scopes/scope-1/navigator');
		await flowApi.searchObjects('scope-1', { q: 'phrase', all_visible: true });
		assert(
			calls.at(-1)!.url.startsWith('/api/v2/flow/scopes/scope-1/search?'),
			'scope-native search'
		);
	}
);
await suite.checkAsync(
	'ticket binds scope, object and existing document; frame protocol stays independent',
	async () => {
		await flowApi.createTicket({
			scope_id: 'scope-1',
			object_id: 'object-1',
			document_id: 'document-1',
			client_id: 'client-1',
			origin: 'https://example.test'
		});
		const ticket = calls.at(-1)!;
		assertEqual(ticket.url, '/api/v2/flow/collab/tickets');
		assertEqual(ticket.body.document_id, 'document-1');
		assertEqual(ticket.body.object_id, 'object-1');
		assert(!('workspace_id' in ticket.body), 'ticket contains no workspace stand-in');
	}
);
await suite.checkAsync('mount removal deletes only the explicit placement URL', async () => {
	await flowApi.removeMount('source-1', 'placement-2');
	assertEqual(calls.at(-1)!.method, 'DELETE');
	assertEqual(calls.at(-1)!.url, '/api/v2/flow/objects/source-1/mounts/placement-2');
});
await suite.checkAsync(
	'navigation pagination is transmitted without changing source scope',
	async () => {
		await flowApi.listTrash('scope-1', { limit: 2, cursor: 'opaque-next' });
		assertEqual(calls.at(-1)!.url, '/api/v2/flow/scopes/scope-1/trash?limit=2&cursor=opaque-next');
		await flowApi.getProjectionLag('scope-1', { limit: 1, cursor: 'lag-next' });
		assertEqual(
			calls.at(-1)!.url,
			'/api/v2/flow/scopes/scope-1/projection-lag?limit=1&cursor=lag-next'
		);
	}
);
suite.check(
	'v2 adopts scope ceilings and refuses a missing scope ceiling without v1 fallback',
	() => {
		const wire: Record<string, unknown> = { version: '2' };
		for (const [field, value] of Object.entries(DEFAULT_FLOW_LIMITS))
			wire[wireFieldName(field as keyof typeof DEFAULT_FLOW_LIMITS)] = value;
		delete wire.connections_per_workspace_max;
		wire.connections_per_scope_max = 17;
		const accepted = negotiateFlowLimitsVersion(wire);
		assertEqual(accepted.readOnly, false);
		assertEqual(accepted.limits.connectionsPerWorkspaceMax, 17);
		delete wire.connections_per_scope_max;
		wire.connections_per_workspace_max = 999;
		const denied = negotiateFlowLimitsVersion(wire);
		assertEqual(denied.readOnly, true);
		assert(
			denied.outcome === 'unknownVersion' &&
				denied.missingFields.includes('connections_per_scope_max'),
			'no cross-version ceiling fallback'
		);
	}
);
globalThis.fetch = originalFetch;
finish(suite.result());
