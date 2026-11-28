<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import BadgeCheck from '@lucide/svelte/icons/badge-check';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import Pagination from '#lib/ui/pagination/Pagination.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TextField from '#lib/ui/text-field/TextField.svelte';
	import EventsPanel from './EventsPanel.svelte';
	import IndexImage from './IndexImage.svelte';
	import { indexApi, type IndexListItem, type IndexSource } from './index-api';
	import { mergeFacet, type Role } from './index-logic';
	import { KINDS, pickText } from './logic';
	import PublicDetail from './PublicDetail.svelte';
	import SourcesPanel from './SourcesPanel.svelte';

	/**
	 * The Public tab of the Market: plugins of the operator's public index sources. Everything is
	 * read from the server's cached copy of each index; members browse, owners and administrators
	 * import (the server decides, the buttons follow its answer and the role they were given).
	 */
	let {
		workspaceId,
		role,
		onimported
	}: {
		workspaceId: string;
		role: Role;
		/** A package was imported (or is already here): open its install flow. */
		onimported: (digest: string) => void;
	} = $props();

	const PAGE_SIZES = [20, 50, 100];

	let sources = $state.raw<IndexSource[]>([]);
	let sourcesLoaded = $state(false);
	let sourcesFailed = $state(false);

	let source = $state<string | null>(null);
	let query = $state('');
	let kind = $state<string | null>(null);
	let category = $state<string | null>(null);
	let label = $state<string | null>(null);
	let compatible = $state(true);
	let page = $state(1);
	let pageSize = $state(20);

	let loading = $state(true);
	let failed = $state(false);
	let items = $state.raw<IndexListItem[]>([]);
	let total = $state(0);
	let categories = $state<string[]>([]);
	let labels = $state<string[]>([]);

	let selected = $state<{ source: string; id: string } | null>(null);
	let showSources = $state(false);
	let showEvents = $state(false);

	const visible = $derived(sources.filter((entry) => !entry.hidden));
	const admin = $derived(role === 'owner' || role === 'admin');
	const filtered = $derived(
		query.trim() !== '' || kind !== null || category !== null || label !== null || !compatible
	);
	const problem = $derived(visible.filter((entry) => entry.stale));

	async function loadSources() {
		const result = await indexApi.sources(workspaceId);
		sourcesLoaded = true;
		sourcesFailed = result.code !== 0;
		sources = result.data ?? [];
		if (source && !sources.some((entry) => entry.id === source && !entry.hidden)) source = null;
	}

	let token = 0;
	async function load() {
		const mine = ++token;
		loading = true;
		const result = await indexApi.plugins(workspaceId, {
			...(source ? { source } : {}),
			q: query.trim(),
			...(kind ? { kind } : {}),
			...(category ? { category } : {}),
			...(label ? { label } : {}),
			compatible,
			page,
			perPage: pageSize
		});
		if (mine !== token) return;
		loading = false;
		failed = result.code !== 0;
		if (failed) {
			items = [];
			total = 0;
			toast.error(describeResult(result, get(t)('market.index.loadFailed')));
			return;
		}
		items = result.data?.items ?? [];
		total = result.data?.total ?? 0;
		categories = mergeFacet(
			categories,
			items.map((item) => item.category)
		);
		labels = mergeFacet(
			labels,
			items.flatMap((item) => item.labels)
		);
	}

	onMount(() => {
		void loadSources();
	});

	// Any filter change goes back to the first page; typing waits for a pause.
	let lastKey = '';
	$effect(() => {
		const key = JSON.stringify([source, query.trim(), kind, category, label, compatible]);
		const reset = key !== lastKey;
		lastKey = key;
		if (reset && page !== 1) {
			page = 1;
			return;
		}
		void [page, pageSize, workspaceId, sourcesLoaded];
		if (!sourcesLoaded) return;
		const timer = setTimeout(() => void load(), reset && query.trim() !== '' ? 300 : 0);
		return () => clearTimeout(timer);
	});

	function titleOf(item: IndexListItem): string {
		return pickText(item.title, $locale ?? 'en') || item.id;
	}

	function publisherName(item: IndexListItem): string {
		return pickText(item.publisher.name, $locale ?? 'en') || item.publisher.id;
	}

	function kindName(value: string): string {
		const key = `market.kindOne.${value}`;
		const text = get(t)(key);
		return text === key ? value : text;
	}

	function facetTitle(prefix: 'category' | 'label', id: string): string {
		const key = `market.index.${prefix}.${id}`;
		const text = get(t)(key);
		return text === key ? id : text;
	}

	function resetFilters() {
		query = '';
		kind = null;
		category = null;
		label = null;
		compatible = true;
	}

	async function afterSourceChange() {
		await loadSources();
		await load();
	}

	const sourceOptions = $derived(
		visible.map((entry) => ({ value: entry.id, label: entry.id, meta: String(entry.plugins) }))
	);
	const kindOptions = $derived(
		KINDS.map((value) => ({ value: value as string, label: $t(`market.kind.${value}`) }))
	);
	const categoryOptions = $derived(
		categories.map((id) => ({ value: id, label: facetTitle('category', id) }))
	);
	const labelOptions = $derived(
		labels.map((id) => ({ value: id, label: facetTitle('label', id) }))
	);
</script>

{#if selected}
	<PublicDetail
		{workspaceId}
		{role}
		source={selected.source}
		pluginId={selected.id}
		onback={() => {
			selected = null;
			void load();
		}}
		{onimported}
	/>
{:else}
	<div class="space-y-5" data-market-public>
		<div class="flex flex-wrap items-end gap-3">
			<div class="min-w-56 flex-1">
				<TextField
					label={$t('market.searchLabel')}
					bind:value={query}
					spellcheck={false}
					maxlength={100}
				/>
			</div>
			{#if visible.length > 1}
				<div class="w-44">
					<Select
						label={$t('market.index.sourceLabel')}
						value={source}
						options={sourceOptions}
						allowEmpty
						placeholder={$t('market.index.allSources')}
						onchange={(value) => (source = value)}
					/>
				</div>
			{/if}
			<div class="w-44">
				<Select
					label={$t('market.kindLabel')}
					value={kind}
					options={kindOptions}
					allowEmpty
					placeholder={$t('market.kindAll')}
					onchange={(value) => (kind = value)}
				/>
			</div>
			<div class="w-44">
				<Select
					label={$t('market.index.categoryLabel')}
					value={category}
					options={categoryOptions}
					allowEmpty
					placeholder={$t('market.index.anyCategory')}
					disabled={categoryOptions.length === 0}
					onchange={(value) => (category = value)}
				/>
			</div>
			<div class="w-44">
				<Select
					label={$t('market.index.labelLabel')}
					value={label}
					options={labelOptions}
					allowEmpty
					placeholder={$t('market.index.anyLabel')}
					disabled={labelOptions.length === 0}
					onchange={(value) => (label = value)}
				/>
			</div>
		</div>

		<div class="flex flex-wrap items-center justify-between gap-3">
			<Checkbox bind:checked={compatible} label={$t('market.index.compatibleOnly')} />
			<div class="flex flex-wrap items-center gap-2">
				{#if problem.length > 0}
					<StatusChip tone="warning" data-market-index-stale
						>{$t('market.index.someStale', { values: { count: problem.length } })}</StatusChip
					>
				{/if}
				<Button
					variant="text"
					size="sm"
					onclick={() => (showSources = !showSources)}
					aria-expanded={showSources}
					data-market-index-sources-toggle>{$t('market.index.sources')}</Button
				>
				{#if admin}
					<Button
						variant="text"
						size="sm"
						onclick={() => (showEvents = !showEvents)}
						aria-expanded={showEvents}
						data-market-index-events-toggle>{$t('market.index.events')}</Button
					>
				{/if}
			</div>
		</div>

		{#if showSources}
			<SourcesPanel {workspaceId} {sources} {role} onchanged={() => void afterSourceChange()} />
		{/if}
		{#if showEvents && admin}
			<EventsPanel {workspaceId} />
		{/if}

		{#if !sourcesLoaded || (loading && items.length === 0)}
			<StateBlock kind="loading" />
		{:else if sourcesFailed || (failed && !loading)}
			<StateBlock
				kind="loadFailed"
				thing={$t('market.index.thing')}
				action={{
					run: () => {
						void loadSources();
						void load();
					}
				}}
			/>
		{:else if sources.length === 0}
			<StateBlock
				kind="empty"
				title={$t('market.index.noneTitle')}
				message={$t('market.index.noneBody')}
			/>
		{:else if visible.length === 0}
			<StateBlock
				kind="empty"
				title={$t('market.index.allHiddenTitle')}
				message={role === 'owner'
					? $t('market.index.allHiddenOwner')
					: $t('market.index.allHiddenMember')}
				action={role === 'owner'
					? { label: $t('market.index.sources'), run: () => (showSources = true) }
					: undefined}
			/>
		{:else if items.length === 0}
			{#if filtered}
				<StateBlock
					kind={query.trim() ? 'noSearchResults' : 'noResults'}
					thing={$t('market.index.thing')}
					query={query.trim()}
					count={[query.trim() !== '', kind, category, label, !compatible].filter(Boolean).length}
					action={{ label: $t('market.index.clearFilters'), run: resetFilters }}
				/>
			{:else}
				<StateBlock
					kind="empty"
					title={$t('market.index.emptyTitle')}
					message={$t('market.index.emptyBody')}
				/>
			{/if}
		{:else}
			<ul class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3" aria-busy={loading}>
				{#each items as item (`${item.source}/${item.id}`)}
					<li data-market-index-plugin={item.id}>
						<button
							type="button"
							class="card"
							onclick={() => (selected = { source: item.source, id: item.id })}
						>
							<span class="head">
								<IndexImage
									{workspaceId}
									source={item.source}
									pluginId={item.id}
									path={item.icon}
									alt=""
									variant="icon"
								/>
								<span class="names">
									<span class="title">{titleOf(item)}</span>
									<span class="publisher">
										{publisherName(item)}
										{#if item.publisher.trusted}
											<BadgeCheck
												size={14}
												aria-label={$t('market.index.trustedPublisher')}
												class="trusted"
											/>
										{/if}
										<span class="id">{item.id}</span>
									</span>
								</span>
							</span>
							<span class="summary">{pickText(item.summary, $locale ?? 'en')}</span>
							<span class="chips">
								<StatusChip>{kindName(item.kind)}</StatusChip>
								{#if item.latest_compatible}
									<StatusChip
										>{$t('market.version', {
											values: { version: item.latest_compatible }
										})}</StatusChip
									>
								{:else}
									<StatusChip tone="warning">{$t('market.index.noCompatible')}</StatusChip>
								{/if}
								{#if item.installed}
									<StatusChip tone="success">{$t('market.installedBadge')}</StatusChip>
								{:else if item.imported}
									<StatusChip tone="success">{$t('market.index.imported')}</StatusChip>
								{/if}
							</span>
						</button>
					</li>
				{/each}
			</ul>
			<Pagination
				{page}
				{pageSize}
				{total}
				{loading}
				pageSizes={PAGE_SIZES}
				onpage={(next) => (page = next)}
				onpagesize={(size) => {
					pageSize = size;
					page = 1;
				}}
				label={$t('market.index.paging')}
			/>
		{/if}
	</div>
{/if}

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		width: 100%;
		height: 100%;
		padding: var(--rf-space-5);
		text-align: left;
		background: var(--rf-bg);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		cursor: pointer;
		transition:
			background var(--rf-duration-fast) var(--rf-ease-standard),
			border-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.card:hover {
		background: var(--rf-hover);
		border-color: var(--rf-line-strong);
	}
	.card:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
	.head {
		display: flex;
		align-items: center;
		gap: var(--rf-space-5);
		min-width: 0;
	}
	.names {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.title {
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-semibold);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.publisher {
		display: flex;
		align-items: center;
		gap: var(--rf-space-2);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		min-width: 0;
	}
	.publisher :global(.trusted) {
		color: var(--rf-success);
		flex: none;
	}
	.id {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.summary {
		font-size: var(--rf-size-secondary);
		color: var(--rf-secondary);
		display: -webkit-box;
		-webkit-line-clamp: 3;
		line-clamp: 3;
		-webkit-box-orient: vertical;
		overflow: hidden;
		min-height: 3.6em;
	}
	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
		margin-top: auto;
	}
</style>
