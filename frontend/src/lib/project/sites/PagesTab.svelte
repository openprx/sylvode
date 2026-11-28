<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import ArrowDown from '@lucide/svelte/icons/arrow-down';
	import ArrowUp from '@lucide/svelte/icons/arrow-up';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { describeResult } from '#lib/i18n/describe.js';
	import { layoutApi } from '#lib/layout/api.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import PageDialog from './PageDialog.svelte';
	import { isPackagePage, moveUpdates, siteTitle } from './view';
	import type { Site, SitePage } from './wire';

	/**
	 * The pages of a site: the site's own, which owners, admins and site editors change, and the
	 * ones a package brings, which only an install, upgrade or revoke changes.
	 */
	let {
		site,
		projectId,
		canEdit
	}: {
		site: Site;
		projectId: string;
		/** Owner, admin or this site's editor; the server decides again. */
		canEdit: boolean;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let problem = $state('');
	let pages = $state.raw<SitePage[]>([]);
	let layoutKeys = $state.raw<Record<string, string>>({});
	let editing = $state<SitePage | 'new' | null>(null);
	let busy = $state(false);

	const packageKey = $derived(site.installation?.package_key ?? '');
	const hasPackagePages = $derived(pages.some(isPackagePage));
	const ownPages = $derived(pages.filter((page) => !isPackagePage(page)));

	async function load(): Promise<void> {
		phase = 'loading';
		const result = await sitesApi.pages(site.id);
		if (result.code !== 0 || !result.data) {
			problem = describeResult(result);
			phase = 'failed';
			return;
		}
		pages = result.data.items;
		phase = 'ready';
		void loadLayoutKeys(result.data.items);
	}

	/** Layout keys for the table: the project's layouts first, then any other layout a page names. */
	async function loadLayoutKeys(items: SitePage[]): Promise<void> {
		const keys: Record<string, string> = {};
		const listed = await layoutApi.listLayouts('project', projectId);
		for (const layout of listed.data?.items ?? []) keys[layout.id] = layout.layout_key;
		const missing = [...new Set(items.map((page) => page.layout_id))].filter((id) => !keys[id]);
		await Promise.all(
			missing.map(async (id) => {
				const layout = await layoutApi.getLayout(id);
				if (layout.code === 0 && layout.data) keys[id] = layout.data.layout_key;
			})
		);
		layoutKeys = keys;
	}

	onMount(() => {
		void load();
	});

	async function move(page: SitePage, step: -1 | 1): Promise<void> {
		const updates = moveUpdates(pages, page.id, step);
		if (updates.length === 0) return;
		busy = true;
		for (const update of updates) {
			const result = await sitesApi.updatePage(update.id, { position: update.position });
			if (result.code !== 0) {
				toast.error(errorText(result, $t));
				break;
			}
		}
		busy = false;
		await load();
	}

	async function remove(page: SitePage): Promise<void> {
		const yes = await confirmAction({
			tone: 'destructive',
			title: $t('sites.pages.deleteTitle', { values: { route: page.route } }),
			body: $t('sites.pages.deleteBody'),
			confirmLabel: $t('sites.pages.deleteConfirm')
		});
		if (!yes) return;
		busy = true;
		const result = await sitesApi.removePage(page.id);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.pages.deleted'));
			await load();
		} else toast.error(errorText(result, $t));
	}

	function saved(): void {
		editing = null;
		toast.success($t('sites.pages.saved'));
		void load();
	}

	const columns = $derived<Column<SitePage>[]>([
		{ id: 'route', header: $t('sites.pages.col.route'), width: 220, cell: routeCell },
		{ id: 'nav', header: $t('sites.pages.col.nav'), width: 180, cell: navCell },
		{ id: 'audience', header: $t('sites.pages.col.audience'), width: 130, cell: audienceCell },
		{ id: 'layout', header: $t('sites.pages.col.layout'), width: 180, cell: layoutCell },
		{ id: 'source', header: $t('sites.pages.col.source'), width: 190, cell: sourceCell },
		...(canEdit
			? [{ id: 'actions', header: '', width: 190, align: 'end' as const, cell: actionsCell }]
			: [])
	]);
</script>

{#snippet routeCell(page: SitePage)}<span class="route">{page.route}</span>{/snippet}
{#snippet navCell(page: SitePage)}
	{#if page.nav !== null}
		{siteTitle(page.nav, $locale)}
	{:else}
		<span class="muted">{$t('sites.pages.notInMenu')}</span>
	{/if}
{/snippet}
{#snippet audienceCell(page: SitePage)}
	<StatusChip tone={page.audience === 'visitor' ? 'neutral' : 'warning'}>
		{$t(`sites.pages.audience_${page.audience}`)}
	</StatusChip>
{/snippet}
{#snippet layoutCell(page: SitePage)}
	{#if layoutKeys[page.layout_id]}{layoutKeys[page.layout_id]}{:else}<span class="muted">…</span
		>{/if}
{/snippet}
{#snippet sourceCell(page: SitePage)}
	{#if isPackagePage(page)}
		<StatusChip>{$t('sites.list.package', { values: { key: packageKey } })}</StatusChip>
	{:else}
		<span class="muted">{$t('sites.list.manual')}</span>
	{/if}
{/snippet}
{#snippet actionsCell(page: SitePage)}
	{#if isPackagePage(page)}
		<span class="muted">{$t('sites.pages.readOnly')}</span>
	{:else}
		{@const at = ownPages.findIndex((own) => own.id === page.id)}
		<span class="actions">
			<IconButton
				icon={ArrowUp}
				label={$t('sites.pages.moveUp')}
				disabled={busy || at <= 0}
				onclick={() => void move(page, -1)}
			/>
			<IconButton
				icon={ArrowDown}
				label={$t('sites.pages.moveDown')}
				disabled={busy || at >= ownPages.length - 1}
				onclick={() => void move(page, 1)}
			/>
			<IconButton
				icon={Pencil}
				label={$t('sites.pages.edit')}
				disabled={busy}
				onclick={() => (editing = page)}
				data-testid="edit-page"
			/>
			<IconButton
				icon={Trash2}
				label={$t('sites.pages.delete')}
				disabled={busy}
				onclick={() => void remove(page)}
				data-testid="delete-page"
			/>
		</span>
	{/if}
{/snippet}

<div class="pages" data-testid="site-pages">
	<div class="bar">
		<p class="lead">{canEdit ? $t('sites.pages.lead') : $t('sites.pages.readOnlyLead')}</p>
		{#if canEdit}
			<Button variant="primary" onclick={() => (editing = 'new')} data-testid="add-page">
				{$t('sites.pages.add')}
			</Button>
		{/if}
	</div>

	{#if hasPackagePages}
		<TipBar tone="neutral" data-testid="package-pages-tip">
			{$t('sites.pages.packageReadOnly', { values: { key: packageKey } })}
			{$t('sites.pages.packageRoutes')}
		</TipBar>
	{/if}

	{#if phase === 'loading'}
		<StateBlock kind="loading" thing={$t('sites.pages.thing')} />
	{:else if phase === 'failed'}
		<StateBlock
			kind="loadFailed"
			thing={$t('sites.pages.thing')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else if pages.length === 0}
		<StateBlock kind="empty" thing={$t('sites.pages.thing')} message={$t('sites.pages.empty')} />
	{:else}
		<DataTable
			rows={pages}
			{columns}
			rowId={(page) => page.id}
			rowLabel={(page) => page.route}
			label={$t('sites.pages.title')}
			thing={$t('sites.pages.thing')}
		/>
	{/if}
</div>

{#if editing}
	<PageDialog
		siteId={site.id}
		{projectId}
		page={editing === 'new' ? undefined : editing}
		currentLayoutKey={editing === 'new' ? undefined : layoutKeys[editing.layout_id]}
		onclose={() => (editing = null)}
		onsaved={saved}
	/>
{/if}

<style>
	.pages {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-5);
	}
	.lead {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.muted {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.actions {
		display: inline-flex;
		gap: var(--rf-space-2);
	}
</style>
