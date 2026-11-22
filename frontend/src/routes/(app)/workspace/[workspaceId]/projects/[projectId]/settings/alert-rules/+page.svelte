<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import RulesSection from '#lib/project/inbound/RulesSection.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<RulesSection {workspaceId} {projectId} />
{/key}
