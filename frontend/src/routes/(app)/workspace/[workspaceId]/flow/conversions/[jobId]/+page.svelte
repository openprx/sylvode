<script lang="ts">
	// Conversion job page (v0.7 Forms Bridge): status, result, warnings and lineage of one
	// Flow -> Forms conversion, with an explicit retry for a failed job. Rendered under the Flow
	// layout, so the navigator and the `flow_enabled` guard still apply. The static `conversions`
	// segment wins over the sibling `[objectId]` route for `/flow/conversions/{job_id}`.
	import { getContext } from 'svelte';
	import { page } from '$app/stores';
	import ConversionJobView from '$lib/components/flow/ConversionJobView.svelte';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import { requireRouteParam } from '$lib/utils/route-params';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commands = new FlowCommandService();

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const jobId = $derived(requireRouteParam($page.params.jobId, 'jobId'));
</script>

<div class="flex-1 overflow-y-auto p-4 md:p-6">
	<div class="mx-auto max-w-3xl">
		{#key `${workspaceId}/${jobId}`}
			<ConversionJobView {workspaceId} {jobId} {repository} {commands} />
		{/key}
	</div>
</div>
