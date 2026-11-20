<script lang="ts">
	import type { Component, Snippet } from 'svelte';
	import Check from '@lucide/svelte/icons/check';
	import { useMenu } from './menu-context';

	/** Data attributes (`data-testid`, ...) are passed to the item element. */
	type DataAttributes = { [K in `data-${string}`]?: string | undefined };

	interface Props extends DataAttributes {
		/** `menuitemradio` and `menuitemcheckbox` carry `aria-checked` and a leading check glyph. */
		role?: 'menuitem' | 'menuitemradio' | 'menuitemcheckbox';
		/** State of a radio or checkbox item; ignored for a plain `menuitem`. */
		checked?: boolean;
		/** Optional 16px Lucide icon. */
		icon?: Component<{ size?: number | string; 'aria-hidden'?: 'true' }>;
		/** Shortcut hint printed at the end of the row, in platform form. */
		shortcut?: string;
		destructive?: boolean;
		/** Disabled items stay focusable (menu pattern) and ignore activation. */
		disabled?: boolean;
		compact?: boolean;
		/** Render a real link: Cmd/Ctrl-click and middle-click open a new browser tab. */
		href?: string;
		/** Keep the menu open after selecting (a checkbox item that toggles several options). */
		keepOpen?: boolean;
		onselect?: () => void;
		/** Content at the end of the row (a dot, a hint). */
		trailing?: Snippet;
		children: Snippet;
	}

	let {
		role = 'menuitem',
		checked = false,
		icon: Icon,
		shortcut,
		destructive = false,
		disabled = false,
		compact = false,
		href,
		keepOpen = false,
		onselect,
		trailing,
		children,
		...rest
	}: Props = $props();

	const menu = useMenu();
	const checkable = $derived(role !== 'menuitem');
	const link = $derived(!!href && !disabled);

	function activate(event: MouseEvent) {
		if (disabled) {
			event.preventDefault();
			return;
		}
		// A modified click on a link belongs to the browser (new tab); the menu stays as it is.
		if (link && (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)) return;
		onselect?.();
		if (!keepOpen) menu?.close();
	}
</script>

{#snippet content()}
	{#if checkable}
		<span class="check"
			>{#if checked}<Check size={16} aria-hidden="true" />{/if}</span
		>
	{:else if Icon && !compact}
		<Icon size={16} aria-hidden="true" />
	{/if}
	<span class="label">{@render children()}</span>
	{#if trailing}{@render trailing()}{/if}
	{#if shortcut}<span class="shortcut">{shortcut}</span>{/if}
{/snippet}

<!-- eslint-disable svelte/no-navigation-without-resolve -- the caller builds the full path -->
{#if link}
	<a
		{...rest}
		{href}
		{role}
		aria-checked={checkable ? checked : undefined}
		tabindex="-1"
		class="ui-menu-item"
		class:compact
		class:destructive
		data-ui="menu-item"
		onclick={activate}
	>
		{@render content()}
	</a>
{:else}
	<button
		{...rest}
		type="button"
		{role}
		aria-checked={checkable ? checked : undefined}
		tabindex="-1"
		aria-disabled={disabled || undefined}
		class="ui-menu-item"
		class:compact
		class:destructive
		data-ui="menu-item"
		onclick={activate}
	>
		{@render content()}
	</button>
{/if}

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
		text-decoration: none;
		cursor: pointer;
	}
	.ui-menu-item.compact {
		min-height: 32px;
		font-size: var(--rf-size-secondary);
	}
	.label {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.check {
		display: inline-grid;
		flex: none;
		width: 16px;
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
		color: var(--rf-secondary);
		cursor: not-allowed;
	}
	.ui-menu-item[aria-disabled='true'] .shortcut {
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.ui-menu-item {
			min-height: var(--rf-touch-target);
		}
	}
</style>
