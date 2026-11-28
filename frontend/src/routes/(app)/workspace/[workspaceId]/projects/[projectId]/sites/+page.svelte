<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import SitesList from '#lib/project/sites/SitesList.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<SitesList {workspaceId} {projectId} />
{/key}
