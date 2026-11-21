<script lang="ts">
	import { t } from 'svelte-i18n';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import type { ScopeKey } from '../destinations';
	import type { Load } from './column-data.svelte';
	import ScopeMenu from './ScopeMenu.svelte';
	import { scopeLabel, type WorkspaceRef } from './scope';

	interface Props {
		scope: ScopeKey;
		workspaces: Load<WorkspaceRef>;
		onselect: (scope: ScopeKey) => void;
		onretry: () => void;
	}

	let { scope, workspaces, onselect, onretry }: Props = $props();

	let open = $state(false);
	let anchor = $state<HTMLElement | null>(null);

	const label = $derived(
		scopeLabel(scope, workspaces.items, {
			personal: $t('shell.column.scope.personal'),
			fallback: $t('shell.column.scope.workspace')
		})
	);
</script>

<button
	type="button"
	class="trigger"
	data-shell="scope-switcher"
	data-scope={scope}
	aria-haspopup="menu"
	aria-expanded={open}
	bind:this={anchor}
	onclick={() => (open = !open)}
>
	<span class="name">{label}</span>
	<ChevronDown size={16} aria-hidden="true" />
</button>
<ScopeMenu
	{open}
	{anchor}
	{scope}
	{workspaces}
	{onretry}
	onclose={() => (open = false)}
	onselect={(next) => {
		open = false;
		onselect(next);
	}}
/>

<style>
	/* 36 high, full width. */
	.trigger {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		flex: 1;
		min-width: 0;
		min-height: calc(var(--rf-space-9) + var(--rf-space-2));
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-sm);
		background: transparent;
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-semibold);
		text-align: start;
		cursor: pointer;
	}
	.trigger:hover {
		background: var(--rf-hover-nav);
	}
	.trigger:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.trigger :global(svg) {
		flex: none;
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.trigger {
			min-height: var(--rf-touch-target);
		}
	}
</style>
