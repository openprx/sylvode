<script lang="ts">
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { projectOptionsStore as appOptionsStore } from '#lib/stores/project-options.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import type { ScopeKey } from '../destinations';
	import type { ColumnData, Load } from './column-data.svelte';
	import { agentsMode, botRows, participantRows, projectsOfWorkspace, showBots } from './lists';
	import NavList from './NavList.svelte';
	import { emptyIdSet, selectedRowId, toggledIds, type NavRow } from './rows';
	import SkeletonRows from './SkeletonRows.svelte';
	import type { ProjectOptionsSource, RouteUrl } from './types';

	interface Props {
		scope: ScopeKey;
		url: RouteUrl;
		/** Owner or admin of `scope`'s workspace: only they may list bots. */
		workspaceAdmin: boolean;
		data: ColumnData;
		onfollow?: (row?: NavRow) => void;
		/** Project option source; the app's store by default. */
		optionsStore?: ProjectOptionsSource;
	}
	let {
		scope,
		url,
		workspaceAdmin,
		data,
		onfollow,
		optionsStore = appOptionsStore
	}: Props = $props();

	const mode = $derived(agentsMode(scope));
	const bots = $derived(showBots(scope, workspaceAdmin));

	onMount(() => void optionsStore.ensureLoaded());

	// --- bots ---
	const botsLoad = $derived<Load<{ id: string; name: string }>>(data.botsOf(scope));
	$effect(() => {
		if (bots) void data.loadBots(scope);
	});
	const botList = $derived(botRows(scope, botsLoad.items));

	// --- AI participants ---
	const options = $derived($optionsStore);
	const projects = $derived(projectsOfWorkspace(options.items, scope, $locale ?? undefined));
	/** Expanded projects; memory only. */
	let expanded = $state.raw<ReadonlySet<string>>(emptyIdSet());
	$effect(() => {
		for (const id of expanded) void data.loadAgents(id);
	});
	const branches = $derived(
		projects.map((project) => {
			const load = data.agentsOf(project.id);
			const rows = participantRows({
				workspaceId: scope,
				projects: [project],
				expanded,
				agentsOf: (id) => data.agentsOf(id).items
			});
			return {
				project,
				load,
				head: rows[0],
				children: rows.slice(1),
				open: expanded.has(project.id)
			};
		})
	);
	const selectedId = $derived(
		selectedRowId([...botList, ...branches.flatMap((b) => [b.head, ...b.children])], url)
	);

	function toggle(id: string, open: boolean) {
		const projectId = id.startsWith('ai-project:') ? id.slice('ai-project:'.length) : null;
		if (!projectId) return;
		expanded = toggledIds(expanded, projectId, open);
	}
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
	<section class="group" data-column-group="participants">
		<p class="caption">{$t('shell.column.agents.participants')}</p>
		{#if !options.loaded && options.loading}
			<SkeletonRows thing={$t('shell.column.things.projects')} />
		{:else if options.error && projects.length === 0}
			<StateBlock
				kind="loadFailed"
				thing={$t('shell.column.things.projects')}
				action={{ run: () => void optionsStore.ensureLoaded(true) }}
			/>
		{:else if options.loaded && projects.length === 0}
			<p class="note">{$t('shell.column.agents.noProjects')}</p>
		{:else}
			{#each branches as branch (branch.project.id)}
				<div class="branch">
					<NavList rows={[branch.head]} {selectedId} ontoggle={toggle} {onfollow} />
					{#if branch.open}
						{#if branch.load.status === 'idle' || branch.load.status === 'loading'}
							<SkeletonRows thing={$t('shell.column.things.participants')} count={2} />
						{:else if branch.load.status === 'error' || branch.load.status === 'forbidden'}
							<StateBlock
								kind={branch.load.status === 'forbidden' ? 'forbidden' : 'loadFailed'}
								thing={$t('shell.column.things.participants')}
								action={{ run: () => void data.loadAgents(branch.project.id, true) }}
							/>
						{:else if branch.children.length === 0}
							<p class="note nested">{$t('shell.column.agents.noParticipants')}</p>
						{:else}
							<NavList rows={branch.children} {selectedId} {onfollow} />
						{/if}
					{/if}
				</div>
			{/each}
		{/if}
	</section>
{/if}

<style>
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.branch {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
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
	.note.nested {
		padding-inline-start: calc(var(--rf-space-4) + var(--rf-space-6));
	}
</style>
