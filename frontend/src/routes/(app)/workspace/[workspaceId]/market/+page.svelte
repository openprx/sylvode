<svelte:options runes />

<script lang="ts">
	import { page } from '$app/state';
	import MarketPage from '#lib/market/MarketPage.svelte';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
</script>

{#key workspaceId}
	<MarketPage {workspaceId} />
{/key}
