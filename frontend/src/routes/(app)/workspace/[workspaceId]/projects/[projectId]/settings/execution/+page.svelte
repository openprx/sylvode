<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import ConnectionSection from '#lib/project/connection/ConnectionSection.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<ConnectionSection {workspaceId} {projectId} />
{/key}
