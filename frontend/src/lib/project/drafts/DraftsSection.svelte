<svelte:options runes />

<script lang="ts">
	import { getContext, onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import Segmented from '#lib/ui/segmented/Segmented.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { draftsApi } from './api';
	import DraftReviewDialog from './DraftReviewDialog.svelte';
	import { STATUS_FILTERS, statusTone, targetLine, type Words } from './view';
	import type { DraftKind, DraftStatus, DraftSummary } from './wire';

	/**
	 * Drafts written by agents for one project. A person who administers the workspace opens a
	 * pending draft to review it; everyone else can read the list. With `siteId` the list is that
	 * site's page and title drafts, and the page heading is left to the caller. With `workspaceId`
	 * and `kind` the list is the workspace's drafts of that kind (package uploads), under a smaller heading.
	 */
	let {
		projectId,
		siteId,
		workspaceId,
		kind
	}: { projectId?: string; siteId?: string; workspaceId?: string; kind?: DraftKind } = $props();

	const words: Words = (key, values) => $t(key, { values });

	// Whether the viewer administers the workspace; the server decides again on every approval.
	const adminGate = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	const isAdmin = $derived(adminGate?.() ?? false);

	let status = $state<DraftStatus>('pending');
	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let items = $state.raw<DraftSummary[]>([]);
	let problem = $state('');
	let openId = $state<string | null>(null);

	async function load(): Promise<void> {
		phase = 'loading';
		const result = workspaceId
			? await draftsApi.listWorkspace(workspaceId, { status, kind })
			: await draftsApi.list(
					projectId ?? '',
					siteId ? { status, kind: 'site_page', perPage: 200 } : { status }
				);
		if (result.code === 0 && result.data) {
			items = siteId
				? result.data.items.filter((draft) => draft.target.site_id === siteId)
				: result.data.items;
			phase = 'ready';
		} else {
			problem = describeResult(result);
			phase = 'failed';
		}
	}

	onMount(() => {
		void load();
	});

	function choose(next: string): void {
		status = next as DraftStatus;
		void load();
	}

	function when(iso: string): string {
		return new Date(iso).toLocaleString($locale ?? undefined);
	}

	function author(draft: DraftSummary): string {
		return draft.author.name ?? draft.author.id;
	}

	function closed(): void {
		openId = null;
		void load();
	}
</script>

<div class="drafts" data-testid="drafts-section">
	{#if workspaceId && kind === 'package_upload'}
		<header class="head">
			<h2>{$t('drafts.uploads.title')}</h2>
			<p class="lead">{$t('drafts.uploads.lead')}</p>
		</header>
	{:else if !siteId}
		<header class="head">
			<h1>{$t('drafts.title')}</h1>
			<p class="lead">{$t('drafts.lead')}</p>
		</header>
	{/if}

	<Segmented
		label={$t('drafts.filter')}
		width={360}
		value={status}
		options={STATUS_FILTERS.map((value) => ({ value, label: $t(`drafts.status.${value}`) }))}
		onchange={choose}
	/>

	{#if phase === 'loading'}
		<StateBlock kind="loading" thing={$t('drafts.thing')} />
	{:else if phase === 'failed'}
		<StateBlock
			kind="loadFailed"
			thing={$t('drafts.thing')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else if items.length === 0}
		<StateBlock kind="empty" thing={$t('drafts.thing')} />
	{:else}
		<table class="list" data-testid="drafts-list">
			<thead>
				<tr>
					<th scope="col">{$t('drafts.col.kind')}</th>
					<th scope="col">{$t('drafts.col.target')}</th>
					<th scope="col">{$t('drafts.col.author')}</th>
					<th scope="col">{$t('drafts.col.state')}</th>
					<th scope="col">{$t('drafts.col.created')}</th>
				</tr>
			</thead>
			<tbody>
				{#each items as draft (draft.id)}
					<tr data-draft-id={draft.id}>
						<td>
							<button type="button" class="open" onclick={() => (openId = draft.id)}>
								{$t(`drafts.kind.${draft.kind}`)}
							</button>
						</td>
						<td>{targetLine(draft, words)}</td>
						<td>{author(draft)}</td>
						<td>
							<StatusChip tone={statusTone(draft.status)}
								>{$t(`drafts.status.${draft.status}`)}</StatusChip
							>
							{#if !draft.valid}
								<StatusChip tone="danger">{$t('drafts.invalid')}</StatusChip>
							{/if}
						</td>
						<td>{when(draft.created_at)}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>

{#if openId}
	<DraftReviewDialog draftId={openId} {isAdmin} onclose={closed} />
{/if}

<style>
	.drafts {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		max-width: 960px;
	}
	h1,
	h2 {
		margin: 0;
		font-size: var(--rf-size-title);
		font-weight: var(--rf-weight-semibold);
	}
	.lead {
		margin: var(--rf-space-2) 0 0;
		color: var(--rf-text-secondary);
		font-size: var(--rf-size-secondary);
	}
	.list {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--rf-size-secondary);
	}
	th,
	td {
		padding: var(--rf-space-4) var(--rf-space-5);
		text-align: start;
		border-bottom: 1px solid var(--rf-border);
		vertical-align: top;
	}
	th {
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text-secondary);
	}
	.open {
		background: none;
		border: 0;
		padding: 0;
		color: var(--rf-accent);
		font: inherit;
		cursor: pointer;
		text-decoration: underline;
	}
</style>
