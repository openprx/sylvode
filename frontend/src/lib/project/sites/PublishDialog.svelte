<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { layoutApi } from '#lib/layout/api.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { layoutBindingKeys, publishSummary, type PublishSummary } from './view';
	import type { Site } from './wire';

	/**
	 * Confirms a publication and says what becomes public. The warnings are computed here and do not
	 * block: the server publishes any site.
	 */
	let {
		site,
		title,
		busy,
		onconfirm,
		onclose
	}: {
		site: Site;
		title: string;
		busy: boolean;
		onconfirm: () => void;
		onclose: () => void;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let summary = $state.raw<PublishSummary | null>(null);

	async function load(): Promise<void> {
		phase = 'loading';
		const [pages, bindings] = await Promise.all([
			sitesApi.pages(site.id),
			sitesApi.bindings(site.id)
		]);
		if (pages.code !== 0 || !pages.data || bindings.code !== 0 || !bindings.data) {
			phase = 'failed';
			return;
		}
		const layoutIds = [...new Set(pages.data.items.map((page) => page.layout_id))];
		const specs: Record<string, string[]> = {};
		await Promise.all(
			layoutIds.map(async (id) => {
				const layout = await layoutApi.getLayout(id);
				if (layout.code === 0 && layout.data) specs[id] = layoutBindingKeys(layout.data.spec);
			})
		);
		const pageBindingKeys: Record<string, string[]> = {};
		for (const page of pages.data.items) pageBindingKeys[page.id] = specs[page.layout_id] ?? [];
		summary = publishSummary({
			pages: pages.data.items,
			bindings: bindings.data.items,
			pageBindingKeys,
			url: site.url
		});
		phase = 'ready';
	}

	onMount(() => {
		void load();
	});
</script>

<Dialog
	open
	title={$t('sites.publish.title', { values: { title } })}
	{onclose}
	testId="publish-dialog"
>
	{#if phase === 'loading'}
		<StateBlock kind="loading" thing={$t('sites.publish.thing')} />
	{:else if phase === 'failed'}
		<TipBar tone="warning" action={{ label: $t('sites.retry'), run: () => void load() }}>
			{$t('sites.publish.summaryFailed')}
		</TipBar>
	{:else if summary}
		<div class="body">
			<p>{$t('sites.publish.lead')}</p>
			<ul class="facts">
				<li>{$t('sites.publish.visitorPages', { values: { count: summary.visitorPages } })}</li>
				{#each summary.publicBindings as binding (binding.key)}
					<li>
						{$t('sites.publish.binding', {
							values: { key: binding.key, submit: binding.submit, read: binding.read }
						})}
					</li>
				{/each}
			</ul>
			{#each summary.warnings as warning, index (index)}
				<TipBar tone="warning" data-testid="publish-warning">
					{#if warning.code === 'missing_binding'}
						{$t('sites.publish.warn.missing_binding', {
							values: { route: warning.route, key: warning.key }
						})}
					{:else}
						{$t(`sites.publish.warn.${warning.code}`)}
					{/if}
				</TipBar>
			{/each}
			<p class="hint">{$t('sites.publish.agents')}</p>
		</div>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} onclick={onclose}>{$t('sites.cancel')}</Button>
		<Button
			variant="primary"
			loading={busy}
			disabled={phase === 'loading'}
			onclick={onconfirm}
			data-testid="confirm-publish"
		>
			{$t('sites.publish.confirm')}
		</Button>
	{/snippet}
</Dialog>

<style>
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	p {
		margin: 0;
	}
	.facts {
		margin: 0;
		padding-inline-start: var(--rf-space-7);
	}
	.hint {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
