<svelte:options runes />

<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
	import SiteDetail from '#lib/project/sites/SiteDetail.svelte';
	import { tabOf, type SiteTab } from '#lib/project/sites/view.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const siteId = $derived(requireRouteParam(page.params.siteId, 'siteId'));
	const tab = $derived(tabOf(page.url.searchParams.get('tab')));

	function choose(next: SiteTab): void {
		const params = new SvelteURLSearchParams(page.url.search);
		params.set('tab', next);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same page, new query
		void goto(`?${params.toString()}`, { replaceState: true, reset: false });
	}
</script>

{#key siteId}
	<SiteDetail {workspaceId} {projectId} {siteId} {tab} ontab={choose} />
{/key}
