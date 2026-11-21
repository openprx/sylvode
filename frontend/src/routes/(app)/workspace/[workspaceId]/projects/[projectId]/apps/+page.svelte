<script lang="ts">
	import { getContext } from 'svelte';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import ProjectPluginNav from '#lib/components/plugins/ProjectPluginNav.svelte';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import { layoutSlot, projectDiscovery, type DiscoveryState } from '#lib/plugins/contributions.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';

	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });

	$effect(() => {
		const current = projectDiscovery(projectId);
		const stop = current.subscribe((value) => (discovery = value));
		const release = current.retain();
		return () => {
			stop();
			release();
		};
	});

	const layout = $derived(layoutSlot(discovery.items, 'project.view'));
	const appCount = $derived(layout.inline.length + layout.more.length + layout.hidden.length);
	const thing = $derived($t('forms.recordPanel.apps').toLowerCase());
</script>

<div class="apps-index" data-testid="project-apps-index">
	<h1>{$t('forms.recordPanel.apps')}</h1>
	{#if discovery.status === 'loading'}
		<StateBlock kind="loading" {thing} />
	{:else if discovery.status === 'error' && discovery.items.length === 0}
		<StateBlock
			kind="loadFailed"
			{thing}
			action={{ run: () => void projectDiscovery(projectId).refresh() }}
		/>
	{:else if appCount > 0 || canManage?.()}
		<ProjectPluginNav {workspaceId} {projectId} />
	{:else}
		<StateBlock kind="empty" {thing} message={$t('plugins.apps.empty')} />
	{/if}
</div>

<style>
	.apps-index {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-8);
		padding: var(--rf-space-9);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	h1 {
		margin: 0;
		font-size: var(--rf-size-page);
		line-height: var(--rf-leading-title);
		font-weight: var(--rf-weight-semibold);
	}
</style>
