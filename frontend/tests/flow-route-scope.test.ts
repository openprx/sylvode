import { flowApi } from '../src/lib/api/flow';
import { load } from '../src/routes/(app)/flow/+layout';
import { Suite, assertEqual, finish } from './support/harness';
const suite = new Suite('flow_route_scope');
const original = {
	getCapabilities: flowApi.getCapabilities,
	getHome: flowApi.getHome,
	getObject: flowApi.getObject,
	getWorkspaceScope: flowApi.getWorkspaceScope
};
const capabilities = {
	block_transfers_enabled: false,
	portable_page_packages_enabled: false,
	portable_graph_packages_enabled: false,
	workspace_editor_initialization: false,
	personal_provisioning: false,
	personal_writes: false,
	external_mounts: false,
	personal_package_imports: false,
	personal_scope_package_exports: false,
	personal_forms_conversion: false,
	cross_workspace_forms_conversion: false
};
const ok = <T>(data: T) => ({ code: 0, message: 'ok', data });
const denied = () => ({
	code: 403,
	message: 'Unavailable',
	data: null,
	error_code: 'feature_disabled'
});
let homeReads = 0;
let legacyReads = 0;
function defaults() {
	homeReads = 0;
	legacyReads = 0;
	flowApi.getCapabilities = async () => ok(capabilities);
	flowApi.getHome = async () => {
		homeReads++;
		return denied();
	};
	flowApi.getObject = async () => ok({ id: 'object', scope_id: 'source-scope' } as never);
	flowApi.getWorkspaceScope = async () => {
		legacyReads++;
		return ok({
			scope: { id: 'legacy-scope', kind: 'workspace', lifecycle_status: 'active' },
			roots: []
		});
	};
}
async function route(params: Record<string, string> = {}) {
	return (await load({ params } as Parameters<typeof load>[0])) as Record<string, unknown>;
}
await suite.checkAsync(
	'existing personal data remains readable when creation switches are off',
	async () => {
		defaults();
		flowApi.getHome = async () =>
			ok({ scope: { id: 'own-scope', kind: 'personal', lifecycle_status: 'active' }, roots: [] });
		const result = await route();
		assertEqual(result.flowEnabled, true, 'existing home reads');
		assertEqual(result.scopeId, 'own-scope', 'real personal scope');
	}
);
await suite.checkAsync(
	'direct shared object works without personal provisioning and never probes source roots',
	async () => {
		defaults();
		const result = await route({ objectId: 'object' });
		assertEqual(result.flowEnabled, true, 'authorized direct object renders');
		assertEqual(result.scopeId, 'source-scope', 'canonical source');
		assertEqual(result.navigationEnabled, false, 'no private source navigator');
		assertEqual(legacyReads, 0, 'no fabricated workspace resolver');
	}
);
await suite.checkAsync(
	'legacy direct link can resolve its real workspace scope without requiring personal home',
	async () => {
		defaults();
		flowApi.getObject = async () =>
			ok({ id: 'object', scope_id: 'legacy-scope', workspace_id: 'workspace' } as never);
		const result = await route({ objectId: 'object' });
		assertEqual(result.workspaceId, 'workspace', 'integration context preserved');
		assertEqual(result.scopeId, 'legacy-scope', 'real binding');
	}
);
await suite.checkAsync(
	'shared source keeps the caller own home as navigation context',
	async () => {
		defaults();
		flowApi.getHome = async () =>
			ok({
				scope: { id: 'caller-scope', kind: 'personal', lifecycle_status: 'active' },
				roots: []
			});
		const result = await route({ objectId: 'object' });
		assertEqual(result.scopeId, 'caller-scope', 'source ancestry is not navigation');
		assertEqual(legacyReads, 0, 'no legacy authority inferred');
	}
);
Object.assign(flowApi, original);
finish(suite.result());
