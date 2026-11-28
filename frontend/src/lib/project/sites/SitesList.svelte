<svelte:options runes />

<!-- eslint-disable svelte/no-navigation-without-resolve -- console paths and the site's own address are built in full here -->

<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve -- console paths are built in full here */
	import { getContext, onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import { projectsApi } from '#lib/api/projects.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import { draftsApi } from '#lib/project/drafts/api.js';
	import Button from '#lib/ui/button/Button.svelte';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import PageHeader from '#lib/ui/page-header/PageHeader.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { sitesApi } from './api';
	import NewSiteDialog from './NewSiteDialog.svelte';
	import { pendingBySite, siteTitle, statusTone } from './view';
	import type { Site } from './wire';

	/** The sites of one project: status, address, source and last publication. */
	let { workspaceId, projectId }: { workspaceId: string; projectId: string } = $props();

	const adminGate = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	const isAdmin = $derived(adminGate?.() ?? false);

	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let items = $state.raw<Site[]>([]);
	let problem = $state('');
	let projectName = $state('');
	let creating = $state(false);
	let waiting = $state.raw(new Map<string, number>());

	const base = $derived(`/workspace/${encodeURIComponent(workspaceId)}`);
	const siteHref = (site: Site) =>
		`${base}/projects/${encodeURIComponent(projectId)}/sites/${encodeURIComponent(site.id)}`;

	async function load(): Promise<void> {
		phase = 'loading';
		const result = await sitesApi.list(projectId);
		if (result.code === 0 && result.data) {
			items = result.data.items;
			phase = 'ready';
			void loadWaiting();
		} else {
			problem = describeResult(result);
			phase = 'failed';
		}
	}

	/** One project-wide list of pending site drafts, counted per site; the column stays empty if it fails. */
	async function loadWaiting(): Promise<void> {
		const result = await draftsApi.list(projectId, {
			status: 'pending',
			kind: 'site_page',
			perPage: 200
		});
		if (result.code === 0 && result.data) waiting = pendingBySite(result.data.items);
	}

	onMount(() => {
		void load();
		void projectsApi.get(projectId).then((project) => (projectName = project.data?.name ?? ''));
	});

	function when(iso: string | null): string {
		return iso ? new Date(iso).toLocaleString($locale ?? undefined) : $t('sites.list.never');
	}

	const columns = $derived<Column<Site>[]>([
		{ id: 'title', header: $t('sites.list.col.title'), width: 240, cell: titleCell },
		{ id: 'status', header: $t('sites.list.col.status'), width: 120, cell: statusCell },
		{ id: 'address', header: $t('sites.list.col.address'), width: 280, cell: addressCell },
		{ id: 'source', header: $t('sites.list.col.source'), width: 180, cell: sourceCell },
		{ id: 'waiting', header: $t('sites.list.col.waiting'), width: 150, cell: waitingCell },
		{ id: 'published', header: $t('sites.list.col.published'), width: 180, cell: publishedCell }
	]);

	const openMarket = () => void goto(`${base}/market`);
</script>

{#snippet titleCell(site: Site)}
	<span class="title">{siteTitle(site.title, $locale)}</span>
	<span class="slug">{site.slug}</span>
{/snippet}
{#snippet statusCell(site: Site)}
	<StatusChip tone={statusTone(site.status)} data-testid="site-status">
		{$t(`sites.status.${site.status}`)}
	</StatusChip>
{/snippet}
{#snippet addressCell(site: Site)}
	{#if site.url}
		<a class="address" href={site.url} target="_blank" rel="noopener noreferrer">
			<span>{site.url}</span>
			<ExternalLink size={14} aria-hidden="true" />
		</a>
	{:else}
		<span class="muted">{$t('sites.list.notServed')}</span>
	{/if}
{/snippet}
{#snippet sourceCell(site: Site)}
	{#if site.installation}
		<StatusChip
			>{$t('sites.list.package', { values: { key: site.installation.package_key } })}</StatusChip
		>
	{:else}
		<span class="muted">{$t('sites.list.manual')}</span>
	{/if}
{/snippet}
{#snippet waitingCell(site: Site)}
	{@const count = waiting.get(site.id) ?? 0}
	{#if count > 0}
		<a class="waiting" href={`${siteHref(site)}?tab=drafts`} data-testid="site-waiting">
			<StatusChip tone="warning">{$t('sites.list.waiting', { values: { count } })}</StatusChip>
		</a>
	{:else}
		<span class="muted">-</span>
	{/if}
{/snippet}
{#snippet publishedCell(site: Site)}{when(site.published_at)}{/snippet}

<div class="sites" data-testid="sites-list">
	<PageHeader
		title={$t('sites.title')}
		context={projectName ? { segments: [], current: projectName } : undefined}
		primary={isAdmin
			? {
					id: 'new',
					label: $t('sites.list.new'),
					kind: 'primary',
					run: () => {
						creating = true;
					}
				}
			: undefined}
	/>

	{#if phase === 'loading'}
		<StateBlock kind="loading" thing={$t('sites.thing')} />
	{:else if phase === 'failed'}
		<StateBlock
			kind="loadFailed"
			thing={$t('sites.thing')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else if items.length === 0}
		<StateBlock kind="empty" thing={$t('sites.thing')} message={$t('sites.list.empty')} />
		<div class="empty-actions">
			{#if isAdmin}
				<Button variant="primary" onclick={() => (creating = true)} data-testid="new-site-empty">
					{$t('sites.list.new')}
				</Button>
			{/if}
			<Button variant="secondary" onclick={openMarket}>{$t('sites.list.browseTemplates')}</Button>
		</div>
	{:else}
		<DataTable
			rows={items}
			{columns}
			rowId={(site) => site.id}
			rowHref={siteHref}
			label={$t('sites.title')}
			thing={$t('sites.thing')}
		/>
	{/if}
</div>

{#if creating}
	<NewSiteDialog
		{workspaceId}
		{projectId}
		onclose={() => (creating = false)}
		oncreated={(site) => void goto(siteHref(site))}
	/>
{/if}

<style>
	.sites {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		padding: var(--rf-space-9);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	.title {
		display: block;
	}
	.slug,
	.muted {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.address {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		color: var(--rf-accent);
		overflow-wrap: anywhere;
	}
	.empty-actions {
		display: flex;
		justify-content: center;
		gap: var(--rf-space-4);
	}
</style>
