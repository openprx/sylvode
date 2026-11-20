<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Check from '@lucide/svelte/icons/check';

	interface Props extends Omit<HTMLButtonAttributes, 'children' | 'class'> {
		/** Selected filters use the `selected` fill and a leading check glyph (specification 3.3). */
		selected?: boolean;
		children: Snippet;
	}

	let { selected = false, type = 'button', children, ...rest }: Props = $props();
</script>

<button
	{...rest}
	{type}
	class="ui-filter-chip"
	class:selected
	aria-pressed={selected}
	data-ui="filter-chip"
	data-selected={selected ? '' : undefined}
>
	{#if selected}<Check size={14} class="check" aria-hidden="true" />{/if}
	<span>{@render children()}</span>
</button>

<style>
	.ui-filter-chip {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		min-height: 28px;
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-chip);
		background: var(--rf-surface);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-regular);
		white-space: nowrap;
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.ui-filter-chip.selected {
		background: var(--rf-selected);
	}
	.ui-filter-chip :global(.check) {
		flex: none;
		color: var(--rf-text);
	}
	.ui-filter-chip:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.ui-filter-chip:disabled {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	@media (pointer: coarse) {
		.ui-filter-chip {
			min-height: var(--rf-touch-target);
		}
	}
</style>
