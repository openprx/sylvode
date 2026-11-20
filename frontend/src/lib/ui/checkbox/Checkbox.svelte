<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLInputAttributes } from 'svelte/elements';
	import { composeHandlers } from '../foundations/compose';
	import Check from '@lucide/svelte/icons/check';
	import Minus from '@lucide/svelte/icons/minus';

	interface Props extends Omit<HTMLInputAttributes, 'class' | 'type' | 'children' | 'checked'> {
		checked?: boolean;
		/** Mixed state; the first user toggle clears it and sets `checked`. */
		indeterminate?: boolean;
		label?: string;
		children?: Snippet;
	}

	let {
		checked = $bindable(false),
		indeterminate = $bindable(false),
		label,
		children,
		disabled = false,
		onchange: onChange,
		...rest
	}: Props = $props();

	let input = $state<HTMLInputElement | null>(null);

	$effect(() => {
		if (input) input.indeterminate = indeterminate;
	});

	/** The internal handler runs first, then the caller's, so neither replaces the other. */
	const onchange = composeHandlers<Event & { currentTarget: EventTarget & HTMLInputElement }>(
		() => (indeterminate = false),
		() => onChange
	);
</script>

<label class="ui-check" class:disabled>
	<span class="control">
		<input
			{...rest}
			bind:this={input}
			bind:checked
			{disabled}
			type="checkbox"
			aria-checked={indeterminate ? 'mixed' : undefined}
			data-ui="checkbox"
			data-filled={(checked || indeterminate) && !disabled ? '' : undefined}
			{onchange}
		/>
		{#if indeterminate}
			<Minus class="glyph" size={13} aria-hidden="true" />
		{:else if checked}
			<Check class="glyph" size={13} aria-hidden="true" />
		{/if}
	</span>
	{#if children}{@render children()}{:else if label}<span class="text">{label}</span>{/if}
</label>

<style>
	.ui-check {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-4);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		color: var(--rf-text);
		cursor: pointer;
	}
	.ui-check.disabled {
		cursor: not-allowed;
	}
	.control {
		position: relative;
		display: inline-grid;
		place-items: center;
		width: 18px;
		height: 18px;
		flex: none;
	}
	input {
		grid-area: 1 / 1;
		appearance: none;
		margin: 0;
		width: 18px;
		height: 18px;
		border: var(--rf-border-width-control) solid var(--rf-control);
		border-radius: var(--rf-radius-xs);
		background: var(--rf-input);
		cursor: inherit;
	}
	input:checked,
	input[aria-checked='mixed'] {
		background: var(--rf-primary);
		border-color: var(--rf-primary);
	}
	input:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	input[data-filled]:focus-visible {
		outline-offset: 2px;
	}
	input:disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
	}
	input:disabled:checked,
	input:disabled[aria-checked='mixed'] {
		background: var(--rf-control);
		border-color: var(--rf-control);
	}
	.control :global(.glyph) {
		grid-area: 1 / 1;
		position: relative;
		color: var(--rf-on-primary);
		pointer-events: none;
	}
	.ui-check.disabled .text {
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.ui-check {
			min-height: var(--rf-touch-target);
		}
	}
</style>
