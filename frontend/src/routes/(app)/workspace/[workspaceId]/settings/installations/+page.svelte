<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import InstallationsPage from '#lib/market/InstallationsPage.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
</script>

{#key workspaceId}
	<InstallationsPage {workspaceId} />
{/key}
