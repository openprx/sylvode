<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { formsApi, type UniversalForm } from '#lib/api/forms.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { layoutApi } from '#lib/layout/api.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import BindingPanel from './BindingPanel.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import { accessOf, layoutBindingKeys, pagesUsing, type TypeOutlets } from './view';
	import type { Site, SiteBinding, SitePage } from './wire';

	/**
	 * What the site's forms show to people outside the workspace: one row per binding. Everybody
	 * reads the list; owners and admins add, change and remove bindings (the server decides again).
	 */
	let {
		site,
		projectId,
		isAdmin
	}: {
		site: Site;
		projectId: string;
		isAdmin: boolean;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let problem = $state('');
	let bindings = $state.raw<SiteBinding[]>([]);
	let forms = $state.raw<UniversalForm[]>([]);
	let outlets = $state.raw<TypeOutlets>({});
	let pages = $state.raw<SitePage[]>([]);
	let pageKeys = $state.raw<Record<string, string[]>>({});
	let editing = $state<SiteBinding | 'new' | null>(null);
	let busy = $state(false);

	const formName = (id: string): string =>
		forms.find((form) => form.id === id)?.name ?? $t('sites.bindings.formUnavailable');

	async function load(): Promise<void> {
		phase = 'loading';
		const [list, formList, types] = await Promise.all([
			sitesApi.bindings(site.id),
			formsApi.list(projectId, { per_page: 200 }),
			formsApi.getFieldTypes()
		]);
		if (list.code !== 0 || !list.data) {
			problem = describeResult(list);
			phase = 'failed';
			return;
		}
		bindings = list.data.items;
		forms = formList.data?.items ?? [];
		outlets = Object.fromEntries(
			(types.data?.types ?? []).map((type) => [type.name, type.outlets])
		);
		phase = 'ready';
		void loadUsage();
	}

	/** Which pages use which binding key, from the page layouts; best effort, for the delete warning. */
	async function loadUsage(): Promise<void> {
		const list = await sitesApi.pages(site.id);
		if (list.code !== 0 || !list.data) return;
		const specs: Record<string, string[]> = {};
		await Promise.all(
			[...new Set(list.data.items.map((page) => page.layout_id))].map(async (id) => {
				const layout = await layoutApi.getLayout(id);
				if (layout.code === 0 && layout.data) specs[id] = layoutBindingKeys(layout.data.spec);
			})
		);
		const keys: Record<string, string[]> = {};
		for (const page of list.data.items) keys[page.id] = specs[page.layout_id] ?? [];
		pages = list.data.items;
		pageKeys = keys;
	}

	onMount(() => {
		void load();
	});

	function saved(): void {
		editing = null;
		toast.success($t('sites.bindings.saved'));
		void load();
	}

	async function remove(binding: SiteBinding): Promise<void> {
		const users = pagesUsing(binding.binding_key, pages, pageKeys);
		const body =
			users.length > 0
				? `${$t('sites.bindings.deleteBody')} ${$t('sites.bindings.deleteUsed', {
						values: { routes: users.map((page) => page.route).join(', ') }
					})}`
				: $t('sites.bindings.deleteBody');
		const yes = await confirmAction({
			tone: 'destructive',
			title: $t('sites.bindings.deleteTitle', { values: { key: binding.binding_key } }),
			body,
			confirmLabel: $t('sites.bindings.deleteConfirm')
		});
		if (!yes) return;
		busy = true;
		const result = await sitesApi.removeBinding(site.id, binding.binding_key);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.bindings.deleted'));
			if (typeof editing === 'object' && editing?.binding_key === binding.binding_key)
				editing = null;
			await load();
		} else toast.error(errorText(result, $t));
	}

	function visitorText(binding: SiteBinding): string {
		const access = accessOf(binding.projection);
		const parts: string[] = [];
		if (access.submit > 0)
			parts.push($t('sites.bindings.summary.submit', { values: { count: access.submit } }));
		if (access.visitorRead > 0)
			parts.push($t('sites.bindings.summary.readOwn', { values: { count: access.visitorRead } }));
		return parts.length > 0 ? parts.join(', ') : $t('sites.bindings.summary.none');
	}

	function operatorText(binding: SiteBinding): string {
		const access = accessOf(binding.projection);
		const parts: string[] = [];
		if (access.operatorRead > 0)
			parts.push($t('sites.bindings.summary.readAll', { values: { count: access.operatorRead } }));
		if (access.update > 0)
			parts.push($t('sites.bindings.summary.update', { values: { count: access.update } }));
		return parts.length > 0 ? parts.join(', ') : $t('sites.bindings.summary.none');
	}

	const columns = $derived<Column<SiteBinding>[]>([
		{ id: 'key', header: $t('sites.bindings.col.key'), width: 180, cell: keyCell },
		{ id: 'form', header: $t('sites.bindings.col.form'), width: 200, cell: formCell },
		{ id: 'visitors', header: $t('sites.bindings.col.visitors'), width: 260, cell: visitorCell },
		{ id: 'operators', header: $t('sites.bindings.col.operators'), width: 240, cell: operatorCell },
		...(isAdmin
			? [{ id: 'actions', header: '', width: 100, align: 'end' as const, cell: actionsCell }]
			: [])
	]);
</script>

{#snippet keyCell(binding: SiteBinding)}<span class="key">{binding.binding_key}</span>{/snippet}
{#snippet formCell(binding: SiteBinding)}{formName(binding.form_id)}{/snippet}
{#snippet visitorCell(binding: SiteBinding)}{visitorText(binding)}{/snippet}
{#snippet operatorCell(binding: SiteBinding)}{operatorText(binding)}{/snippet}
{#snippet actionsCell(binding: SiteBinding)}
	<span class="actions">
		<IconButton
			icon={Pencil}
			label={$t('sites.bindings.edit')}
			disabled={busy}
			onclick={() => (editing = binding)}
			data-testid="edit-binding"
		/>
		<IconButton
			icon={Trash2}
			label={$t('sites.bindings.delete')}
			disabled={busy}
			onclick={() => void remove(binding)}
			data-testid="delete-binding"
		/>
	</span>
{/snippet}

<div class="split">
	<div class="bindings" data-testid="site-bindings">
		<div class="bar">
			<p class="lead">{isAdmin ? $t('sites.bindings.lead') : $t('sites.bindings.readOnlyLead')}</p>
			{#if isAdmin}
				<Button
					variant="primary"
					disabled={phase !== 'ready'}
					onclick={() => (editing = 'new')}
					data-testid="add-binding"
				>
					{$t('sites.bindings.add')}
				</Button>
			{/if}
		</div>

		{#if phase === 'loading'}
			<StateBlock kind="loading" thing={$t('sites.bindings.thing')} />
		{:else if phase === 'failed'}
			<StateBlock
				kind="loadFailed"
				thing={$t('sites.bindings.thing')}
				message={problem || undefined}
				action={{ run: () => void load() }}
			/>
		{:else if bindings.length === 0}
			<StateBlock
				kind="empty"
				thing={$t('sites.bindings.thing')}
				message={$t('sites.bindings.empty')}
			/>
		{:else}
			{#if site.status === 'published'}
				<TipBar tone="warning">{$t('sites.bindings.publishedTip')}</TipBar>
			{/if}
			<DataTable
				rows={bindings}
				{columns}
				rowId={(binding) => binding.binding_key}
				rowLabel={(binding) => binding.binding_key}
				label={$t('sites.bindings.title')}
				thing={$t('sites.bindings.thing')}
			/>
		{/if}
	</div>

	{#if editing && phase === 'ready'}
		{#key editing === 'new' ? 'new' : editing.binding_key}
			<BindingPanel
				siteId={site.id}
				published={site.status === 'published'}
				{forms}
				{outlets}
				takenKeys={bindings.map((binding) => binding.binding_key)}
				binding={editing === 'new' ? undefined : editing}
				onclose={() => (editing = null)}
				onsaved={saved}
			/>
		{/key}
	{/if}
</div>

<style>
	.split {
		display: flex;
		align-items: stretch;
		gap: var(--rf-space-5);
		min-height: 0;
	}
	.bindings {
		display: flex;
		flex: 1;
		flex-direction: column;
		gap: var(--rf-space-5);
		min-width: 0;
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
	.key {
		font-family: var(--rf-font-mono-stack, monospace);
	}
	.actions {
		display: inline-flex;
		gap: var(--rf-space-2);
	}
</style>
