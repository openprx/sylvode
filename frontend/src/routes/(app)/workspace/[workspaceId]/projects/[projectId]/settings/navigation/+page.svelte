<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import NavEditor from '#lib/nav/editor/NavEditor.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<NavEditor scope="project" scopeId={projectId} {workspaceId} />
{/key}
