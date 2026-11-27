<script lang="ts">
	import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
	import { locale, t } from 'svelte-i18n';
	import CustomiseSheet from '#lib/nav/CustomiseSheet.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import { navStore } from '#lib/nav/store.svelte.js';
	import { workspaceGroups } from '#lib/nav/rows.js';
	import { PERSONAL_SCOPE, type FlowAvailability } from '../destinations';
	import type { ShellState } from '../shell-state.svelte';
	import { homeRows, recentRows, type Translate } from './lists';
	import NavList from './NavList.svelte';
	import { selectedRowId, type NavRow } from './rows';
	import type { RouteUrl } from './types';

	interface Props {
		shell: ShellState;
		url: RouteUrl;
		unread: number;
		flow?: FlowAvailability;
		onfollow?: (row?: NavRow) => void;
	}
	let {
		shell,
		url,
		unread,
		flow = { personal: false, workspaces: {} },
		onfollow
	}: Props = $props();

	const tr: Translate = (key, values) => $t(key, values ? { values } : undefined);
	const workspaceId = $derived(shell.scope === PERSONAL_SCOPE ? null : shell.scope);

	// The workspace navigation loads with the scope; until it has, and when the endpoint is not
	// there, the built-in rows below show.
	$effect(() => {
		if (workspaceId) void navStore.load('workspace', workspaceId);
	});
	const menu = $derived(workspaceId ? navStore.state('workspace', workspaceId).menu : null);

	const recent = $derived(recentRows(shell.recent));
	const groups = $derived(
		menu && workspaceId
			? workspaceGroups(menu, {
					workspaceId,
					locale: $locale ?? 'en',
					tr,
					unread,
					flow,
					recent
				})
			: null
	);
	const main = $derived(homeRows({ unread, tr }));
	let customising = $state(false);

	/** Groups the person opened or closed against the configured default (memory only). */
	let flipped = $state.raw<readonly string[]>([]);
	function toggleGroup(id: string) {
		flipped = flipped.includes(id) ? flipped.filter((f) => f !== id) : [...flipped, id];
	}

	const allRows = $derived(groups ? groups.flatMap((g) => g.rows) : [...main, ...recent]);
	const selectedId = $derived(selectedRowId(allRows, url));
</script>

{#if groups && workspaceId}
	<div class="tools">
		<IconButton
			icon={SlidersHorizontal}
			label={$t('nav.customise.open')}
			data-testid="nav-customise-home"
			onclick={() => (customising = true)}
		/>
	</div>
	{#if customising}
		<CustomiseSheet
			open
			scope="workspace"
			scopeId={workspaceId}
			{workspaceId}
			onclose={() => (customising = false)}
		/>
	{/if}
	{#each groups as group (group.id)}
		{@const open = group.collapsed === flipped.includes(group.id)}
		<div class="group" data-column-group={group.id}>
			{#if group.caption}
				<button
					type="button"
					class="caption caption-button"
					aria-expanded={open}
					onclick={() => toggleGroup(group.id)}
				>
					{group.caption}
				</button>
			{/if}
			{#if open || !group.caption}
				<NavList rows={group.rows} {selectedId} {onfollow} />
			{/if}
		</div>
	{/each}
{:else}
	<div class="group" data-column-group="home">
		<NavList rows={main} {selectedId} {onfollow} />
	</div>
	{#if recent.length > 0}
		<div class="group" data-column-group="recent">
			<p class="caption">{$t('shell.column.home.recent')}</p>
			<NavList rows={recent} {selectedId} {onfollow} />
		</div>
	{/if}
{/if}

<style>
	.tools {
		display: flex;
		justify-content: flex-end;
		padding-inline: var(--rf-space-4);
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.caption {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-4) var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-medium);
	}
	.caption-button {
		border: 0;
		background: none;
		text-align: start;
		cursor: pointer;
		font-family: inherit;
	}
	.caption-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
</style>
