<script lang="ts">
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import CreatorPluginLink from './CreatorPluginLink.svelte';
	import PluginPreferenceMenu from './PluginPreferenceMenu.svelte';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import type { PluginContributionItem } from '#lib/api/plugin-packages.js';
	import {
		appHref,
		type DiscoveryState,
		itemKey,
		layoutSlot,
		projectDiscovery
	} from '#lib/plugins/contributions.js';

	let { workspaceId, projectId }: { workspaceId: string; projectId: string } = $props();
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
	const visible = $derived(discovery.status === 'ready' || discovery.status === 'error');
	const href = (item: PluginContributionItem) =>
		appHref(workspaceId, projectId, item.key, item.contribution.route ?? '');
</script>

{#if visible && (discovery.items.length > 0 || canManage?.())}
	<section
		class="mb-8 rounded-card border border-line bg-raised p-5"
		aria-labelledby="project-plugin-nav-title"
		data-plugin-slot="project.view"
	>
		<div class="mb-4 flex flex-wrap items-center justify-between gap-2">
			<h2
				id="project-plugin-nav-title"
				class="text-lg font-semibold text-fg"
			>
				{$t('plugins.apps.navTitle')}
			</h2>
			<div class="flex flex-wrap items-center gap-2">
				<PluginPreferenceMenu {projectId} items={discovery.items} />
				<CreatorPluginLink {workspaceId} {projectId} slot="project.view" formKey={null} />
			</div>
		</div>
		{#if layout.inline.length || layout.more.length}
			<ul class="grid gap-3 md:grid-cols-3">
				{#each [...layout.inline, ...layout.more] as item (itemKey(item))}
					<li>
						<a
							href={href(item)}
							class="block rounded-card border border-line p-4 transition-colors hover:border-line-strong hover:bg-surface"
							data-plugin-contribution={item.contribution.id}
							aria-label={$t('plugins.apps.open', { values: { label: item.contribution.label } })}
						>
							<span class="block text-sm font-semibold text-fg"
								>{item.contribution.label}</span
							>
							<span class="mt-1 block text-sm text-fg-muted">
								{$t('plugins.apps.publisher', {
									values: { name: item.name, version: item.version }
								})}
							</span>
						</a>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="text-sm text-fg-muted">{$t('plugins.apps.empty')}</p>
		{/if}
		{#if layout.hidden.length}
			<p class="mt-3 text-xs text-fg-muted">
				{$t('plugins.apps.hiddenCount', { values: { count: layout.hidden.length } })}
			</p>
		{/if}
	</section>
{/if}
