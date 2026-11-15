<script lang="ts">
	import { getContext } from 'svelte';
	import { page } from '$app/state';
	import ConversionJobView from '#lib/components/flow/ConversionJobView.svelte';
	import { FLOW_REPOSITORY_CONTEXT } from '#lib/flow/context-keys.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import { FlowCommandService } from '#lib/flow/command-service.js';
	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commands = new FlowCommandService();
</script>

<div class="flex-1 overflow-y-auto p-4 md:p-6">
	<div class="mx-auto max-w-3xl">
		{#if page.params.jobId}{#key page.params.jobId}<ConversionJobView
					workspaceId={null}
					jobId={page.params.jobId}
					{repository}
					{commands}
				/>{/key}{/if}
	</div>
</div>
