<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import ArrowUpRight from '@lucide/svelte/icons/arrow-up-right';
	import X from '@lucide/svelte/icons/x';
	import { goto } from '$app/navigation';
	import '#lib/ui/foundations/visually-hidden.css';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import { roving } from '#lib/ui/foundations/roving.js';
	import { defaultColumnApis } from '../column/column-apis';
	import { ColumnData } from '../column/column-data.svelte';
	import { columnWidth, isNarrowWindow } from '../column/layout';
	import ScopeMenu from '../column/ScopeMenu.svelte';
	import type { RouteContext } from '../route-map';
	import type { ShellState } from '../shell-state.svelte';
	import type { SettingsViewer } from '../settings-sections';
	import { closeTarget, settingsNavModel } from './frame';

	interface Props {
		ctx: RouteContext;
		shell: ShellState;
		viewer: SettingsViewer;
		/** Window width in px, as for the secondary column. */
		windowWidth: number;
		/** Workspace list loader shared with the column; created here when absent. */
		data?: ColumnData;
		/** Navigation; defaults to `goto`. */
		onnavigate?: (href: string) => void;
	}
	let {
		ctx,
		shell,
		viewer,
		windowWidth,
		data = new ColumnData(defaultColumnApis),
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- the caller builds the full path
		onnavigate = (href) => void goto(href)
	}: Props = $props();

	const narrow = $derived(isNarrowWindow(windowWidth));
	const model = $derived(settingsNavModel(viewer, ctx));
	let chooseOpen = $state(false);
	let chooseAnchor = $state<HTMLElement | null>(null);

	onMount(() => void data.loadWorkspaces());

	function follow() {
		if (narrow) shell.showContent();
	}
</script>

<nav
	class="settings"
	aria-label={$t('shell.destinations.settings')}
	data-shell="settings-column"
	style:width={narrow ? '100%' : `${columnWidth(windowWidth)}px`}
>
	<div class="top">
		<IconButton
			icon={X}
			label={$t('shell.settings.close')}
			data-shell="close-settings"
			onclick={() => onnavigate(closeTarget(shell.settingsReturn))}
		/>
	</div>
	<div
		class="groups"
		use:roving={{
			selector: '[data-settings-section], [data-settings-choose]',
			orientation: 'vertical',
			loop: false
		}}
	>
		{#each model as group (group.id)}
			<section class="group" data-settings-group={group.id}>
				<h2 class="caption">
					{$t(group.labelKey)}
					{#if group.caption}<span class="sub">{group.caption}</span>{/if}
				</h2>
				{#if group.chooseWorkspace}
					<button
						type="button"
						class="item"
						data-settings-choose
						aria-haspopup="menu"
						aria-expanded={chooseOpen}
						bind:this={chooseAnchor}
						onclick={() => (chooseOpen = !chooseOpen)}
					>
						{$t('shell.sections.chooseWorkspace')}
					</button>
				{:else}
					<ul class="list">
						{#each group.sections as section (section.id)}
							<li>
								<!-- eslint-disable svelte/no-navigation-without-resolve -- hrefs are built by the shell -->
								<a
									class="item"
									class:selected={section.selected}
									href={section.href}
									data-settings-section={section.id}
									aria-current={section.selected ? 'page' : undefined}
									onclick={follow}
								>
									<span class="label">{$t(section.labelKey)}</span>
									{#if section.external}
										<ArrowUpRight size={14} aria-hidden="true" />
										<span class="ui-sr-only">{$t('shell.settings.leavesSettings')}</span>
									{/if}
								</a>
								<!-- eslint-enable svelte/no-navigation-without-resolve -->
							</li>
						{/each}
					</ul>
				{/if}
			</section>
		{/each}
	</div>
</nav>
<ScopeMenu
	open={chooseOpen}
	anchor={chooseAnchor}
	scope={shell.scope}
	workspaces={data.workspaces}
	onclose={() => (chooseOpen = false)}
	onselect={(next) => {
		chooseOpen = false;
		shell.setScope(next);
	}}
	onretry={() => void data.loadWorkspaces(true)}
/>

<style>
	.settings {
		display: flex;
		flex-direction: column;
		flex: none;
		gap: var(--rf-space-5);
		box-sizing: border-box;
		min-height: 0;
		padding: 28px var(--rf-space-7);
		background: var(--rf-nav);
		border-inline-end: var(--rf-border-width) solid var(--rf-line);
		font-family: var(--rf-font-sans-stack);
	}
	.top {
		display: flex;
		justify-content: flex-end;
	}
	.groups {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		flex: 1;
		min-height: 0;
		overflow-y: auto;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.caption {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-2);
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-5) var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-medium);
	}
	.sub {
		font-weight: var(--rf-weight-regular);
	}
	.list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
	}
	.item {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		width: 100%;
		box-sizing: border-box;
		padding: var(--rf-space-4) var(--rf-space-5);
		border: 0;
		border-radius: var(--rf-radius-xs);
		background: transparent;
		color: var(--rf-text);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		text-align: start;
		text-decoration: none;
		cursor: pointer;
	}
	.item :global(svg) {
		flex: none;
		color: var(--rf-secondary);
	}
	.item:hover {
		background: var(--rf-hover-nav);
	}
	.item.selected {
		background: var(--rf-selected);
	}
	.item.selected::before {
		content: '';
		position: absolute;
		inset-inline-start: 0;
		inset-block: calc(50% - var(--rf-space-4));
		width: var(--rf-border-width-focus);
		background: var(--rf-primary);
	}
	.item:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.label {
		flex: 1;
		min-width: 0;
	}
	@media (pointer: coarse) {
		.item {
			min-height: var(--rf-touch-target);
		}
	}
</style>
