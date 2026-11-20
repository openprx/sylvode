<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		id: string;
		text: string;
		placeholder?: string;
		disabled?: boolean;
		readonly?: boolean;
		invalid?: boolean;
		required?: boolean;
		compact?: boolean;
		/** Fixed box width in px; the box fills its container otherwise. */
		width?: number;
		name?: string;
		/** Accessible name when the visible label belongs to a group. */
		label?: string;
		describedBy?: string;
		popupId?: string;
		expanded?: boolean;
		onkeydown?: (event: KeyboardEvent) => void;
		onblur?: () => void;
		trailing?: Snippet;
		box?: HTMLElement | null;
		input?: HTMLInputElement | null;
	}

	let {
		id,
		text = $bindable(''),
		placeholder,
		disabled = false,
		readonly = false,
		invalid = false,
		required = false,
		compact = false,
		width,
		name,
		label,
		describedBy,
		popupId,
		expanded = false,
		onkeydown,
		onblur,
		trailing,
		box = $bindable(null),
		input = $bindable(null)
	}: Props = $props();
</script>

<div
	bind:this={box}
	class="box"
	class:compact
	class:invalid
	class:disabled
	class:readonly
	class:open={expanded}
	style:width={width ? `${width}px` : undefined}
>
	<input
		bind:this={input}
		bind:value={text}
		{id}
		{name}
		{placeholder}
		{disabled}
		{readonly}
		type="text"
		role="combobox"
		autocomplete="off"
		spellcheck="false"
		aria-haspopup="dialog"
		aria-expanded={expanded}
		aria-controls={expanded ? popupId : undefined}
		aria-label={label}
		aria-invalid={invalid || undefined}
		aria-required={required || undefined}
		aria-describedby={describedBy}
		{onkeydown}
		{onblur}
		data-ui="date-input"
	/>
	{#if trailing}{@render trailing()}{/if}
</div>

<style>
	.box {
		display: flex;
		align-items: center;
		gap: var(--rf-space-2);
		box-sizing: border-box;
		min-width: 0;
		height: var(--rf-field-height);
		padding: 0 var(--rf-space-3) 0 var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
	}
	.box.compact {
		height: 28px;
	}
	/* The ring replaces the border: drawn on the box's own edge. */
	.box:focus-within,
	.box.open {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.box.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
	.box.disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
		color: var(--rf-secondary);
	}
	.box.readonly {
		background: var(--rf-surface);
		border-color: transparent;
	}
	input {
		flex: 1;
		min-width: 0;
		height: 100%;
		padding: 0;
		border: 0;
		outline: 0;
		background: transparent;
		color: inherit;
		font: inherit;
		font-size: var(--rf-size-body);
		font-variant-numeric: tabular-nums;
	}
	.compact input {
		font-size: var(--rf-size-secondary);
	}
	input::placeholder {
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.box {
			min-height: var(--rf-touch-target);
		}
	}
</style>
