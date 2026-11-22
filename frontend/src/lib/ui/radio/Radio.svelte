<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLInputAttributes } from 'svelte/elements';
	import { composeHandlers } from '../foundations/compose';
	import { useRadioGroup } from './radio-context';

	interface Props extends Omit<HTMLInputAttributes, 'class' | 'type' | 'children' | 'value'> {
		value: string;
		label?: string;
		children?: Snippet;
	}

	let { value, label, children, disabled = false, onchange: onChange, ...rest }: Props = $props();

	const group = useRadioGroup();
	const isDisabled = $derived(disabled || (group?.disabled() ?? false));
	const checked = $derived(group?.value() === value);

	/** The group selection runs first, then the caller's handler. */
	const onchange = composeHandlers<Event & { currentTarget: EventTarget & HTMLInputElement }>(
		() => group?.select(value),
		() => onChange
	);
</script>

<label class="ui-radio" class:disabled={isDisabled}>
	<span class="control">
		<input
			{...rest}
			type="radio"
			name={group?.name}
			{value}
			{checked}
			disabled={isDisabled}
			data-ui="radio"
			{onchange}
		/>
	</span>
	{#if children}{@render children()}{:else if label}<span class="text">{label}</span>{/if}
</label>

<style>
	.ui-radio {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-4);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		color: var(--rf-text);
		cursor: pointer;
	}
	.ui-radio.disabled {
		cursor: not-allowed;
	}
	.control {
		display: inline-grid;
		flex: none;
	}
	input {
		appearance: none;
		margin: 0;
		display: grid;
		place-items: center;
		width: 18px;
		height: 18px;
		border: var(--rf-border-width-control) solid var(--rf-control);
		border-radius: var(--rf-radius-full);
		background: var(--rf-input);
		cursor: inherit;
	}
	/* 9px dot inside the 1.5px primary ring. */
	input:checked {
		border-color: var(--rf-primary);
	}
	input:checked::after {
		content: '';
		width: 9px;
		height: 9px;
		border-radius: var(--rf-radius-full);
		background: var(--rf-primary);
	}
	/* Checked radios are filled controls: the ring sits outside the edge. */
	input:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	input:checked:focus-visible {
		outline-offset: 2px;
	}
	input:disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
	}
	input:disabled:checked::after {
		background: var(--rf-control);
	}
	.ui-radio.disabled .text {
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.ui-radio {
			min-height: var(--rf-touch-target);
		}
	}
</style>
