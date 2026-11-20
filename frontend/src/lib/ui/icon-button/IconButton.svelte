<script lang="ts">
	import type { Component } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import { tooltip } from '../tooltip/tooltip';

	interface Props extends Omit<HTMLButtonAttributes, 'children' | 'class' | 'aria-label'> {
		/** A Lucide icon component. */
		icon: Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;
		/** Accessible name and tooltip text; required because the button has no visible text. */
		label: string;
		/** Printed after the label in the tooltip, in platform form ("Ctrl K"). */
		shortcut?: string;
		/** Header tool clusters draw the glyph in the text colour instead of the secondary one. */
		tone?: 'secondary' | 'text';
		/** Glyph size: 18 by default, 20 in headers. */
		glyph?: 18 | 20;
		/** Toggle state for a tool button; omit for a plain action. */
		pressed?: boolean;
	}

	let {
		icon: Icon,
		label,
		shortcut,
		tone = 'secondary',
		glyph = 18,
		pressed,
		disabled = false,
		type = 'button',
		...rest
	}: Props = $props();
</script>

<button
	{...rest}
	{type}
	{disabled}
	aria-label={label}
	aria-pressed={pressed}
	class="ui-icon-button tone-{tone}"
	data-ui="icon-button"
	use:tooltip={{ text: label, shortcut }}
>
	<Icon size={glyph} aria-hidden="true" />
</button>

<style>
	.ui-icon-button {
		display: inline-grid;
		place-items: center;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.tone-secondary {
		color: var(--rf-secondary);
	}
	.tone-text {
		color: var(--rf-text);
	}
	.ui-icon-button:hover:not(:disabled) {
		background: var(--rf-hover);
	}
	.ui-icon-button:active:not(:disabled),
	.ui-icon-button[aria-pressed='true'] {
		background: var(--rf-pressed);
	}
	.ui-icon-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.ui-icon-button:disabled {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	@media (pointer: coarse) {
		.ui-icon-button {
			width: var(--rf-touch-target);
			height: var(--rf-touch-target);
		}
	}
</style>
