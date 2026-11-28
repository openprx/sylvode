<svelte:options runes />

<script lang="ts">
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { indexApi, type IndexSource } from './index-api';
	import { retryAfter, shortDate, type Role } from './index-logic';

	/**
	 * Status of every configured index source: when it was last fetched, whether the copy is stale
	 * and why, and, for the workspace owner, refresh now (the server allows one a minute per
	 * source) and hide or show it. A hidden source disappears from the Public tab and cannot be
	 * imported from, but its revocations (keys and versions) still reach this workspace.
	 */
	let {
		workspaceId,
		sources,
		role,
		onchanged
	}: {
		workspaceId: string;
		sources: IndexSource[];
		role: Role;
		onchanged: () => void;
	} = $props();

	const owner = $derived(role === 'owner');
	let busy = $state<string | null>(null);
	/** Sources whose refresh was refused for the cooldown, with the second the wait ends. */
	let cooldown = $state<Record<string, number>>({});
	let now = $state(Date.now());

	$effect(() => {
		if (Object.keys(cooldown).length === 0) return;
		const timer = setInterval(() => {
			now = Date.now();
			const next = Object.fromEntries(Object.entries(cooldown).filter(([, until]) => until > now));
			if (Object.keys(next).length !== Object.keys(cooldown).length) cooldown = next;
		}, 1000);
		return () => clearInterval(timer);
	});

	function waitLeft(id: string): number {
		const until = cooldown[id];
		return until ? Math.max(0, Math.ceil((until - now) / 1000)) : 0;
	}

	async function refresh(id: string) {
		busy = id;
		const result = await indexApi.refresh(workspaceId, id);
		busy = null;
		if (result.code === 429) {
			const wait = retryAfter(result.details) ?? 60;
			now = Date.now();
			cooldown = { ...cooldown, [id]: now + wait * 1000 };
			toast.error(get(t)('market.index.refreshWait', { values: { seconds: wait } }));
			return;
		}
		if (result.code !== 0) {
			toast.error(describeResult(result, get(t)('market.index.refreshFailed')));
			return;
		}
		if (result.data?.outcome === 'failed') toast.error(get(t)('market.index.refreshFailed'));
		else if (result.data?.outcome === 'skipped') toast.info(get(t)('market.index.refreshSkipped'));
		else toast.success(get(t)('market.index.refreshed'));
		// A started refresh is the owner's one per minute even when it found nothing new.
		now = Date.now();
		cooldown = { ...cooldown, [id]: now + 60_000 };
		onchanged();
	}

	async function setHidden(id: string, hidden: boolean) {
		busy = id;
		const result = await indexApi.setHidden(workspaceId, id, hidden);
		busy = null;
		if (result.code !== 0) {
			toast.error(describeResult(result, get(t)('market.index.hideFailed')));
			return;
		}
		onchanged();
	}

	function fetched(source: IndexSource): string {
		return source.fetched_at
			? $t('market.index.fetchedAt', {
					values: { when: shortDate(source.fetched_at, $locale ?? 'en') }
				})
			: $t('market.index.neverFetched');
	}
</script>

<section class="panel" aria-label={$t('market.index.sources')} data-market-index-sources>
	<ul class="rows">
		{#each sources as source (source.id)}
			<li class="row" data-market-index-source={source.id}>
				<div class="info">
					<div class="line">
						<span class="id">{source.id}</span>
						{#if source.hidden}
							<StatusChip>{$t('market.index.hidden')}</StatusChip>
						{/if}
						{#if source.error}
							<StatusChip tone="danger">{$t('market.index.statusError')}</StatusChip>
						{:else if source.stale}
							<StatusChip tone="warning">{$t('market.index.statusStale')}</StatusChip>
						{:else}
							<StatusChip tone="success">{$t('market.index.statusFresh')}</StatusChip>
						{/if}
						<span class="meta"
							>{$t('market.index.pluginCount', { values: { count: source.plugins } })}</span
						>
					</div>
					<div class="meta url">{source.url}</div>
					<div class="meta">{fetched(source)}</div>
					{#if source.error}
						<p class="problem" role="status">
							{$t('market.index.errorLine', { values: { error: source.error } })}
							{#if source.fetched_at}
								{$t('market.index.keepingCopy', {
									values: { when: shortDate(source.fetched_at, $locale ?? 'en') }
								})}
							{/if}
						</p>
					{:else if source.stale && source.fetched_at}
						<p class="problem" role="status">{$t('market.index.staleLine')}</p>
					{/if}
					{#if source.warnings.length > 0}
						<p class="meta">
							{$t('market.index.warnings', { values: { count: source.warnings.length } })}
						</p>
					{/if}
					{#if source.hidden && owner}
						<p class="meta">{$t('market.index.hiddenNote')}</p>
					{/if}
				</div>
				{#if owner}
					<div class="actions">
						<Button
							variant="secondary"
							size="sm"
							loading={busy === source.id}
							disabled={waitLeft(source.id) > 0}
							disabledReason={waitLeft(source.id) > 0
								? $t('market.index.refreshIn', { values: { seconds: waitLeft(source.id) } })
								: undefined}
							onclick={() => void refresh(source.id)}
							data-market-index-refresh={source.id}>{$t('market.index.refreshNow')}</Button
						>
						<Button
							variant="text"
							size="sm"
							disabled={busy === source.id}
							onclick={() => void setHidden(source.id, !source.hidden)}
							data-market-index-hide={source.id}
							>{source.hidden ? $t('market.index.show') : $t('market.index.hide')}</Button
						>
					</div>
				{/if}
			</li>
		{/each}
	</ul>
	{#if owner}
		<p class="meta foot">{$t('market.index.hideRevocations')}</p>
	{/if}
</section>

<style>
	.panel {
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		padding: var(--rf-space-5);
		font-family: var(--rf-font-sans-stack);
	}
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		justify-content: space-between;
		gap: var(--rf-space-5);
	}
	.row + .row {
		padding-top: var(--rf-space-5);
		border-top: var(--rf-border-width) solid var(--rf-line);
	}
	.info {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
		flex: 1;
	}
	.line {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-3);
	}
	.id {
		font-weight: var(--rf-weight-semibold);
		font-size: var(--rf-size-body);
	}
	.meta {
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.url {
		overflow-wrap: anywhere;
	}
	.problem {
		margin: 0;
		font-size: var(--rf-size-secondary);
		color: var(--rf-warning);
		max-width: 70ch;
	}
	.actions {
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-3);
	}
	.foot {
		margin: var(--rf-space-5) 0 0;
		max-width: 70ch;
	}
</style>
