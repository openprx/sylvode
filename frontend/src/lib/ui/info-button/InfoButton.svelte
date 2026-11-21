<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import Info from '@lucide/svelte/icons/info';
	import { tooltip } from '../tooltip/tooltip';
	import InfoPopover from './InfoPopover.svelte';

	interface Props extends Omit<HTMLButtonAttributes, 'children' | 'class' | 'aria-label'> {
		/** Accessible name and tooltip text, for example "About approvals". */
		label: string;
		/** The full explanation, shown in the popover (progressive disclosure, specification 3.11). */
		children: Snippet;
	}

	let { label, children, type = 'button', ...rest }: Props = $props();

	let open = $state(false);
	let anchor = $state<HTMLElement | null>(null);
</script>

<button
	{...rest}
	{type}
	bind:this={anchor}
	class="ui-info-button"
	aria-label={label}
	aria-haspopup="dialog"
	aria-expanded={open}
	data-ui="info-button"
	use:tooltip={{ text: label }}
	onclick={() => (open = !open)}
>
	<Info size={16} aria-hidden="true" />
</button>
<InfoPopover {open} {anchor} {label} onclose={() => (open = false)}>
	{@render children()}
</InfoPopover>

<style>
	/* 24 by 24, the glyph at 16 in the secondary colour (specification 2.1). */
	.ui-info-button {
		position: relative;
		display: inline-grid;
		place-items: center;
		width: 24px;
		height: 24px;
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.ui-info-button:hover {
		background: var(--rf-hover);
	}
	.ui-info-button:active,
	.ui-info-button[aria-expanded='true'] {
		background: var(--rf-pressed);
	}
	.ui-info-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	/* On a coarse pointer the hit area grows to 44 without changing the drawing. */
	@media (pointer: coarse) {
		.ui-info-button::before {
			content: '';
			position: absolute;
			inset: -10px;
		}
	}
</style>
