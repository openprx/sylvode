import { FLOW_PROCESS_READER_COMPATIBILITY } from '#lib/flow/product-mode.js';
import { routeFailure } from '#lib/flow/route-failure.js';
import { isConversionJobId } from '#lib/flow/conversion-job.js';
import { flowApi } from '#lib/api/flow.js';
import type { LayoutLoad } from './$types';

export const load: LayoutLoad = async ({ params }) => {
	const capabilities = await flowApi.getCapabilities();
	if (
		capabilities.code !== 0 ||
		capabilities.data?.api_version !== 2 ||
		capabilities.data.minimum_reader_version > FLOW_PROCESS_READER_COMPATIBILITY
	)
		return {
			flowEnabled: false,
			...routeFailure(capabilities),
			error:
				capabilities.data?.minimum_reader_version &&
				capabilities.data.minimum_reader_version > FLOW_PROCESS_READER_COMPATIBILITY
					? 'upgrade'
					: capabilities.message
		};
	let objectId = params.objectId;
	if (!objectId && params.jobId) {
		// The job view owns its safe empty state. Only an authorized job may select
		// a source object or provide navigation context.
		if (!isConversionJobId(params.jobId)) return { flowEnabled: true, navigationEnabled: false };
		const job = await flowApi.getConversion(params.jobId);
		if (job.code === 404 || job.code === 403) return { flowEnabled: true, navigationEnabled: false };
		if (job.code !== 0 || !job.data) return { flowEnabled: false, ...routeFailure(job) };
		objectId = job.data.source_object_id;
	}
	// An object-centric link is authorized independently from personal provisioning. The caller's
	// own home is navigation context only; a shared source never reveals its ancestors or siblings.
	if (objectId) {
		const object = await flowApi.getObject(objectId);
		if (object.code !== 0 || !object.data) return { flowEnabled: false, ...routeFailure(object) };
		const home = await flowApi.getHome();
		if (home.code === 0 && home.data) return { flowEnabled: true, scopeId: home.data.scope.id };
		if (object.data.workspace_id) {
			const legacy = await flowApi.getWorkspaceScope(object.data.workspace_id);
			if (legacy.code === 0 && legacy.data)
				return {
					flowEnabled: true,
					scopeId: legacy.data.scope.id,
					workspaceId: object.data.workspace_id
				};
		}
		return { flowEnabled: true, scopeId: object.data.scope_id, navigationEnabled: false };
	}
	const home = await flowApi.getHome();
	return {
		flowEnabled: home.code === 0 && !!home.data,
		scopeId: home.data?.scope.id,
		...(home.code !== 0 ? routeFailure(home) : {})
	};
};
