<script lang="ts">
	import { t } from 'svelte-i18n';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import type { ScopeKey } from '../destinations';
	import type { ColumnData, Load } from './column-data.svelte';
	import { agentsMode, botRows, showBots } from './lists';
	import NavList from './NavList.svelte';
	import { selectedRowId, type NavRow } from './rows';
	import SkeletonRows from './SkeletonRows.svelte';
	import type { RouteUrl } from './types';

	interface Props {
		scope: ScopeKey;
		url: RouteUrl;
		/** Owner or admin of `scope`'s workspace: only they may list bots. */
		workspaceAdmin: boolean;
		data: ColumnData;
		onfollow?: (row?: NavRow) => void;
	}
	let { scope, url, workspaceAdmin, data, onfollow }: Props = $props();

	const mode = $derived(agentsMode(scope));
	const bots = $derived(showBots(scope, workspaceAdmin));

	// --- bots ---
	const botsLoad = $derived<Load<{ id: string; name: string }>>(data.botsOf(scope));
	$effect(() => {
		if (bots) void data.loadBots(scope);
	});
	const botList = $derived(botRows(scope, botsLoad.items));

	const selectedId = $derived(selectedRowId(botList, url));
</script>

{#if mode === 'personal'}
	<StateBlock
		kind="empty"
		thing={$t('shell.column.things.agents')}
		message={$t('shell.column.agents.personalEmpty')}
	/>
{:else}
	{#if bots}
		<section class="group" data-column-group="bots">
			<p class="caption">{$t('shell.column.agents.bots')}</p>
			{#if botsLoad.status === 'idle' || botsLoad.status === 'loading'}
				<SkeletonRows thing={$t('shell.column.things.bots')} count={2} />
			{:else if botsLoad.status === 'error' || botsLoad.status === 'forbidden'}
				<StateBlock
					kind={botsLoad.status === 'forbidden' ? 'forbidden' : 'loadFailed'}
					thing={$t('shell.column.things.bots')}
					action={{ run: () => void data.loadBots(scope, true) }}
				/>
			{:else if botList.length === 0}
				<p class="note">{$t('shell.column.agents.noBots')}</p>
			{:else}
				<NavList rows={botList} {selectedId} {onfollow} />
			{/if}
		</section>
	{/if}
	<section class="group" data-column-group="connected">
		<p class="caption">{$t('shell.column.agents.connected')}</p>
		<p class="note">{$t('shell.column.agents.noConnection')}</p>
	</section>
{/if}

<style>
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
	.note {
		margin: 0;
		padding: var(--rf-space-2) var(--rf-space-4);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
</style>
