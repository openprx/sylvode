<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import DraftsSection from '#lib/project/drafts/DraftsSection.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
</script>

{#key projectId}
	<DraftsSection {projectId} />
{/key}
