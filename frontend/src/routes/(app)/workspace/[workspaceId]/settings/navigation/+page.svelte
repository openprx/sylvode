<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import NavEditor from '#lib/nav/editor/NavEditor.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
</script>

{#key workspaceId}
	<NavEditor scope="workspace" scopeId={workspaceId} {workspaceId} />
{/key}
