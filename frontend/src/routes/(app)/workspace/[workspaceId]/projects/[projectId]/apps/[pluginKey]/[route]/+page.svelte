<script lang="ts">
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import ProjectPluginView from '#lib/components/plugins/ProjectPluginView.svelte';
	import { parseState, projectHref, safeFrom } from '#lib/plugins/contributions.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const pluginKey = $derived(requireRouteParam(page.params.pluginKey, 'pluginKey'));
	const route = $derived(requireRouteParam(page.params.route, 'route'));
	const parsed = $derived(parseState(page.url.searchParams.get('state')));
	const safe = $derived(safeFrom(page.url.searchParams.get('from'), workspaceId, projectId));
	// Only an explicit, validated return path is kept; the project page is the implicit default.
	const from = $derived(safe === projectHref(workspaceId, projectId) ? null : safe);
</script>

<div class="mx-auto max-w-6xl p-4 md:p-6">
	{#if parsed.tooLong}
		<p class="mb-3 text-sm text-slate-600 dark:text-slate-300" role="status">
			{$t('plugins.apps.stateTooLong')}
		</p>
	{/if}
	{#key `${workspaceId}/${projectId}/${pluginKey}/${route}/${parsed.state ?? ''}`}
		<ProjectPluginView
			{workspaceId}
			{projectId}
			{pluginKey}
			{route}
			routeState={parsed.state}
			{from}
		/>
	{/key}
</div>
