<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import SourcesSection from '#lib/project/inbound/SourcesSection.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<SourcesSection {workspaceId} {projectId} />
{/key}
