<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import LayoutPage from '#lib/nav/LayoutPage.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const layoutId = $derived(requireRouteParam(page.params.layoutId, 'layoutId'));
</script>

<LayoutPage {workspaceId} {projectId} {layoutId} />
