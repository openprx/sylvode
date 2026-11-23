<script lang="ts">
	import { t } from 'svelte-i18n';
	import PanelLeftClose from '@lucide/svelte/icons/panel-left-close';
	import Search from '@lucide/svelte/icons/search';
	import { goto } from '$app/navigation';
	import { isFlowCanvasRoute } from '#lib/flow/page-layout.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import { roving } from '#lib/ui/foundations/roving.js';
	import { DESTINATIONS, type ScopeKey } from '../destinations';
	import type { Destination, RouteContext } from '../route-map';
	import type { Shortcut } from '../shortcuts.svelte';
	import type { ShellState } from '../shell-state.svelte';
	import { defaultColumnApis } from './column-apis';
	import { ColumnData } from './column-data.svelte';
	import FlowColumn from './FlowColumn.svelte';
	import GovernanceColumn from './GovernanceColumn.svelte';
	import HomeColumn from './HomeColumn.svelte';
	import { columnWidth, isNarrowWindow, searchHint } from './layout';
	import ProjectsColumn from './ProjectsColumn.svelte';
	import ScopeSwitcher from './ScopeSwitcher.svelte';
	import { scopeChangeTarget } from './scope';
	import type { ColumnViewer, RouteUrl } from './types';

	interface Props {
		destination: Destination;
		ctx: RouteContext;
		url: RouteUrl;
		/** SvelteKit route id of the current page (decides the Flow canvas default). */
		routeId?: string | null;
		shell: ShellState;
		viewer: ColumnViewer;
		/** Window width in px: 264 wide from 1280, 232 below, full width at 800 or less. */
		windowWidth: number;
		/** Request loader; created here when absent. Re-create it when the user changes. */
		data?: ColumnData;
		/** Opens the command palette. */
		onsearch?: () => void;
		/** "P" on a pinnable row. */
		onpin?: (shortcut: Shortcut) => void;
		/** Navigation after a scope change; defaults to `goto`. */
		onnavigate?: (href: string) => void;
	}

	let {
		destination,
		ctx,
		url,
		routeId = null,
		shell,
		viewer,
		windowWidth,
		data = new ColumnData(defaultColumnApis),
		onsearch,
		onpin,
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- the caller builds the full path
		onnavigate = (href) => void goto(href)
	}: Props = $props();

	const narrow = $derived(isNarrowWindow(windowWidth));
	const collapsed = $derived(
		!narrow && shell.isCollapsed(destination, { flowCanvas: isFlowCanvasRoute(routeId) })
	);
	const scope = $derived<ScopeKey>(shell.scope);
	const label = $derived($t(DESTINATIONS[destination].labelKey));
	const hint = $derived(searchHint(typeof navigator === 'undefined' ? '' : navigator.platform));

	$effect(() => {
		void data.loadWorkspaces();
	});

	function chooseScope(next: ScopeKey) {
		shell.setScope(next);
		const href = scopeChangeTarget({ ctx, scope: next, flow: viewer.flow });
		if (href) onnavigate(href);
	}

	function follow() {
		if (narrow) shell.showContent();
	}
</script>

{#if collapsed}
	<div
		class="placeholder"
		data-shell="column"
		data-destination={destination}
		data-collapsed="true"
	></div>
{:else}
	<div
		class="column"
		data-shell="column"
		data-destination={destination}
		data-collapsed="false"
		style:width={narrow ? '100%' : `${columnWidth(windowWidth)}px`}
	>
		<div class="head">
			<ScopeSwitcher
				{scope}
				workspaces={data.workspaces}
				onselect={chooseScope}
				onretry={() => void data.loadWorkspaces(true)}
			/>
			{#if !narrow}
				<IconButton
					icon={PanelLeftClose}
					label={$t('shell.column.collapse')}
					data-shell="collapse-column"
					onclick={() => shell.setCollapsed(destination, true)}
				/>
			{/if}
		</div>
		<button type="button" class="search" data-shell="search" onclick={() => onsearch?.()}>
			<Search size={16} aria-hidden="true" />
			<span class="placeholder-text">{$t('shell.column.search')}</span>
			<kbd>{hint}</kbd>
		</button>
		<nav
			class="list"
			aria-label={label}
			data-shell-list
			use:roving={{ selector: '[data-nav-row]', orientation: 'vertical', loop: false }}
		>
			{#if destination === 'home'}
				<HomeColumn {shell} {url} unread={viewer.unread} onfollow={follow} />
			{:else if destination === 'projects'}
				<ProjectsColumn {shell} {scope} {url} {ctx} {viewer} {data} {onpin} onfollow={follow} />
			{:else if destination === 'flow'}
				<FlowColumn {scope} {url} {viewer} {data} onfollow={follow} />
			{:else}
				<GovernanceColumn {url} instanceAdmin={viewer.instanceAdmin} onfollow={follow} />
			{/if}
		</nav>
	</div>
{/if}

<style>
	.placeholder {
		width: 0;
		flex: none;
	}
	.column {
		display: flex;
		flex-direction: column;
		flex: none;
		gap: var(--rf-space-6);
		box-sizing: border-box;
		min-height: 0;
		padding: var(--rf-space-8) var(--rf-space-5);
		background: var(--rf-nav);
		border-inline-end: var(--rf-border-width) solid var(--rf-line);
		font-family: var(--rf-font-sans-stack);
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--rf-space-2);
	}
	.search {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		min-height: calc(var(--rf-space-9) + var(--rf-space-1));
		padding: 0 var(--rf-space-4);
		border: var(--rf-border-width-control) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-secondary);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		text-align: start;
		cursor: pointer;
	}
	.search:hover {
		background: var(--rf-hover-nav);
	}
	.search:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.placeholder-text {
		flex: 1;
		min-width: 0;
	}
	kbd {
		font-family: var(--rf-font-latin);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
	@media (pointer: coarse) {
		.search {
			min-height: var(--rf-touch-target);
		}
	}
</style>
