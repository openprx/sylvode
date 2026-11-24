<svelte:options runes />

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { t } from 'svelte-i18n';
	import PageHeader from '#lib/ui/page-header/PageHeader.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import type { LoadState } from './load-state';

	interface Props {
		/** Message keys of the section: its title, description line and the loaded thing. */
		titleKey: string;
		descriptionKey: string;
		thingKey: string;
		state: LoadState;
		/** False shows the "switched off on this server" tip bar above the content. */
		inboundEnabled?: boolean;
		testId?: string;
		onretry: () => void;
		/** Rendered once the load is ready (and no state block replaces it); the filled-in sections pass it. */
		children?: Snippet;
	}
	let {
		titleKey,
		descriptionKey,
		thingKey,
		state,
		inboundEnabled = true,
		testId,
		onretry,
		children
	}: Props = $props();
</script>

<div class="section" data-testid={testId}>
	<PageHeader title={$t(titleKey)} />
	<p class="description">{$t(descriptionKey)}</p>

	{#if state === 'loading'}
		<StateBlock kind="loading" thing={$t(thingKey)} />
	{:else if state === 'error'}
		<StateBlock kind="loadFailed" thing={$t(thingKey)} action={{ run: onretry }} />
	{:else if state === 'forbidden'}
		<StateBlock kind="forbidden" message={$t('inbound.forbidden')} />
	{:else}
		{#if !inboundEnabled}
			<TipBar tone="warning">{$t('inbound.disabledOnServer')}</TipBar>
		{/if}
		{@render children?.()}
	{/if}
</div>

<style>
	.section {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 800px;
	}
	.description {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
