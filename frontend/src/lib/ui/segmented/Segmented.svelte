<script lang="ts" generics="V extends string = string">
	import { roving } from '../foundations/roving';
	import '../foundations/visually-hidden.css';
	import { uid } from '../foundations/ids';
	import { tabStopIndex, type SegmentOption } from './segmented-state';

	interface Props {
		options: SegmentOption<V>[];
		value?: V | null;
		onchange?: (value: V) => void;
		/** Accessible name of the group; or use `labelledby`. */
		label?: string;
		labelledby?: string;
		/** Track width in px (240 by default) or `fill` to take the width of the container. */
		width?: number | 'fill';
		disabled?: boolean;
		id?: string;
	}

	let {
		options,
		value = $bindable(null),
		onchange,
		label,
		labelledby,
		width = 240,
		disabled = false,
		id = uid('segmented')
	}: Props = $props();

	const tabStop = $derived(tabStopIndex(options, value));

	function choose(option: SegmentOption<V>) {
		if (disabled || option.disabled || option.value === value) return;
		value = option.value;
		onchange?.(option.value);
	}
</script>

<!-- One tab stop; arrow keys move focus and selection together (radio group pattern). -->
<div
	{id}
	role="radiogroup"
	aria-label={label}
	aria-labelledby={labelledby}
	aria-disabled={disabled || undefined}
	class="ui-segmented"
	class:fill={width === 'fill'}
	style:width={typeof width === 'number' ? `${width}px` : undefined}
	data-ui="segmented"
	use:roving={{
		selector: '[role="radio"]',
		orientation: 'horizontal',
		onmove: (el) => el.click()
	}}
>
	{#each options as option, index (option.value)}
		{@const off = disabled || option.disabled}
		<button
			type="button"
			role="radio"
			class="segment"
			aria-checked={option.value === value}
			aria-disabled={off || undefined}
			title={off ? option.disabledReason : undefined}
			tabindex={index === tabStop ? 0 : -1}
			data-value={option.value}
			onclick={() => choose(option)}
		>
			<span class="text">{option.label}</span>
			{#if off && option.disabledReason}<span class="ui-sr-only">{option.disabledReason}</span>{/if}
		</button>
	{/each}
</div>

<style>
	.ui-segmented {
		display: inline-flex;
		gap: var(--rf-space-1);
		box-sizing: border-box;
		height: var(--rf-control-height);
		padding: var(--rf-space-1);
		border-radius: var(--rf-radius-sm);
		background: var(--rf-surface);
		font-family: var(--rf-font-sans-stack);
	}
	.ui-segmented.fill {
		display: flex;
		width: 100%;
	}
	.segment {
		flex: 1 1 0;
		min-width: 0;
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-xs);
		background: transparent;
		color: var(--rf-secondary);
		font: inherit;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-regular);
		line-height: var(--rf-leading-secondary);
		cursor: pointer;
	}
	.text {
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.segment[aria-checked='true'] {
		background: var(--rf-surface-white);
		color: var(--rf-text);
		font-weight: var(--rf-weight-semibold);
	}
	.segment:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.segment[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	@media (pointer: coarse) {
		.ui-segmented {
			height: var(--rf-touch-target);
		}
	}
</style>
