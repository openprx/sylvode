<script lang="ts">
	// Convert a Flow Page into a Universal Forms record (v0.7 Forms Bridge). Rendered under the
	// Flow layout, so the navigator and the `flow_enabled` guard still apply. The source object is
	// read through the shared `FlowObjectRepository` without opening a collaboration session.
	import { getContext } from 'svelte';
	import { page } from '$app/stores';
	import ConvertWizard from '$lib/components/flow/ConvertWizard.svelte';
	import { FlowCommandService } from '$lib/flow/command-service';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import { requireRouteParam } from '$lib/utils/route-params';

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commands = new FlowCommandService();

	const workspaceId = $derived(requireRouteParam($page.params.workspaceId, 'workspaceId'));
	const objectId = $derived(requireRouteParam($page.params.objectId, 'objectId'));
</script>

<div class="flex-1 overflow-y-auto p-4 md:p-6">
	<div class="mx-auto max-w-3xl">
		{#key `${workspaceId}/${objectId}`}
			<ConvertWizard {workspaceId} {objectId} {repository} {commands} />
		{/key}
	</div>
</div>
