<script module lang="ts">
	export const CARD_PADDING = { none: '', sm: 'pad-sm', md: 'pad-md', lg: 'pad-lg' } as const;
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		hover?: boolean;
		clickable?: boolean;
		padding?: 'none' | 'sm' | 'md' | 'lg';
		onclick?: () => void;
		children?: Snippet;
	}

	let { hover = false, clickable = false, padding = 'md', onclick, children }: Props = $props();

	const pad = $derived(CARD_PADDING[padding]);
	const interactive = $derived(hover || clickable);
</script>

{#if clickable && onclick}
	<button type="button" {onclick} class="ui-card {pad}" class:interactive class:clickable>
		{@render children?.()}
	</button>
{:else}
	<div class="ui-card {pad}" class:interactive class:clickable>
		{@render children?.()}
	</div>
{/if}

<style>
	.ui-card {
		display: block;
		box-sizing: border-box;
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
	button.ui-card {
		width: 100%;
		text-align: start;
		font-size: inherit;
	}
	.pad-sm {
		padding: var(--rf-space-6);
	}
	.pad-md {
		padding: var(--rf-space-8);
	}
	.pad-lg {
		padding: var(--rf-space-9);
	}
	.interactive {
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.interactive:hover {
		background: var(--rf-hover);
	}
	.clickable {
		cursor: pointer;
	}
	button.ui-card:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
</style>
