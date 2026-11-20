<script module lang="ts">
	export type EmptyStateIcon = 'clipboard' | 'box' | 'search' | 'office';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import Building2 from '@lucide/svelte/icons/building-2';
	import ClipboardList from '@lucide/svelte/icons/clipboard-list';
	import Package from '@lucide/svelte/icons/package';
	import Search from '@lucide/svelte/icons/search';

	interface Props {
		icon?: EmptyStateIcon;
		title: string;
		description?: string;
		action?: Snippet;
	}

	let { icon = 'clipboard', title, description, action }: Props = $props();

	const Glyph = $derived(
		{ clipboard: ClipboardList, box: Package, search: Search, office: Building2 }[icon] ??
			ClipboardList
	);
</script>

<!-- Same anatomy as StateBlock (glyph, title, body, one action) with the caller's own wording. -->
<div class="ui-empty-state" data-ui="empty-state">
	<Glyph size={24} class="glyph" aria-hidden="true" />
	<h3 class="title">{title}</h3>
	{#if description}<p class="body">{description}</p>{/if}
	{#if action}<div class="action">{@render action()}</div>{/if}
</div>

<style>
	.ui-empty-state {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-4);
		padding: var(--rf-space-9) var(--rf-space-6);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
		text-align: center;
		font-family: var(--rf-font-sans-stack);
	}
	.ui-empty-state :global(.glyph) {
		color: var(--rf-secondary);
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.body {
		margin: 0;
		max-width: 360px;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.action {
		display: flex;
		justify-content: center;
		margin-top: var(--rf-space-2);
	}
</style>
