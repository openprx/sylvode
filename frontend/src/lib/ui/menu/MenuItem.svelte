<script lang="ts">
	import type { Component, Snippet } from 'svelte';
	import { useMenu } from './menu-context';

	interface Props {
		/** Optional 16px Lucide icon. */
		icon?: Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;
		/** Shortcut hint printed at the end of the row, in platform form. */
		shortcut?: string;
		destructive?: boolean;
		/** Disabled items stay focusable (menu pattern) and ignore activation. */
		disabled?: boolean;
		compact?: boolean;
		onselect?: () => void;
		children: Snippet;
	}

	let {
		icon: Icon,
		shortcut,
		destructive = false,
		disabled = false,
		compact = false,
		onselect,
		children
	}: Props = $props();

	const menu = useMenu();

	function activate() {
		if (disabled) return;
		onselect?.();
		menu?.close();
	}
</script>

<button
	type="button"
	role="menuitem"
	tabindex="-1"
	aria-disabled={disabled || undefined}
	class="ui-menu-item"
	class:compact
	class:destructive
	data-ui="menu-item"
	onclick={activate}
>
	{#if Icon && !compact}<Icon size={16} aria-hidden="true" />{/if}
	<span class="label">{@render children()}</span>
	{#if shortcut}<span class="shortcut">{shortcut}</span>{/if}
</button>

<style>
	.ui-menu-item {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		width: 100%;
		min-height: var(--rf-menu-item-height);
		padding: 0 var(--rf-space-5);
		border: 0;
		border-radius: var(--rf-radius-sm);
		background: transparent;
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		font-weight: var(--rf-weight-regular);
		text-align: start;
		cursor: pointer;
	}
	.ui-menu-item.compact {
		min-height: 32px;
		font-size: var(--rf-size-secondary);
	}
	.label {
		flex: 1;
		min-width: 0;
	}
	.shortcut {
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.ui-menu-item:hover:not([aria-disabled='true']) {
		background: var(--rf-surface);
	}
	.ui-menu-item:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.destructive {
		color: var(--rf-danger);
	}
	.ui-menu-item[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	.ui-menu-item[aria-disabled='true'] .shortcut {
		color: var(--rf-control);
	}
	@media (pointer: coarse) {
		.ui-menu-item {
			min-height: var(--rf-touch-target);
		}
	}
</style>
