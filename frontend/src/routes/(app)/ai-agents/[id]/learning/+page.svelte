<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		governanceExtApi,
		type AiAlignmentStats,
		type AiLearningRecord
	} from '#lib/api/governance-ext.js';
	import { toast } from '#lib/stores/toast.js';

	const participantId = $derived(page.params.id || '');

	let loading = $state(true);
	let loadingFeedback = $state(false);
	let records = $state<AiLearningRecord[]>([]);
	let stats = $state<AiAlignmentStats | null>(null);
	let domain = $state('');
	let pageNum = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);
	let feedbackReviewId = $state('');
	let feedbackItems = $state<AiLearningRecord[]>([]);

	onMount(() => {
		void loadAll();
	});

	async function loadAll() {
		loading = true;
		const [learningRes, statsRes] = await Promise.all([
			governanceExtApi.getAiParticipantLearning(participantId, {
				domain: domain.trim() || undefined,
				page: pageNum,
				per_page: perPage
			}),
			governanceExtApi.getAiParticipantAlignmentStats(participantId)
		]);

		if (learningRes.code !== 0 || !learningRes.data) {
			toast.error(learningRes.message || get(t)('governanceExt.learningLoadFailed'));
			records = [];
			total = 0;
			totalPages = 0;
		} else {
			records = learningRes.data.items;
			total = learningRes.data.total;
			totalPages = learningRes.data.total_pages;
		}

		if (statsRes.code !== 0 || !statsRes.data) {
			toast.error(statsRes.message || get(t)('governanceExt.learningLoadFailed'));
			stats = null;
		} else {
			stats = statsRes.data;
		}
		loading = false;
	}

	async function loadFeedback(reviewId: string) {
		loadingFeedback = true;
		feedbackReviewId = reviewId;
		const res = await governanceExtApi.getAiReviewFeedback(reviewId);
		loadingFeedback = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('governanceExt.learningLoadFailed'));
			feedbackItems = [];
			return;
		}
		feedbackItems = res.data.items;
	}

	async function search() {
		pageNum = 1;
		await loadAll();
	}

	async function goPage(next: number) {
		pageNum = Math.max(1, Math.min(next, totalPages || 1));
		await loadAll();
	}

	function pct(value: number): string {
		return `${(value * 100).toFixed(1)}%`;
	}

	function tone(value: string): string {
		if (value === 'aligned') return 'bg-surface text-fg';
		if (value === 'misaligned') return 'bg-danger-soft text-danger';
		return 'bg-surface text-fg';
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.aiLearning')}</title>
</svelte:head>

<div class="mx-auto max-w-7xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<h1 class="text-xl font-semibold text-fg">
			{$t('governanceExt.learningTitle')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">
			{$t('governanceExt.learningSubtitle', { values: { id: participantId } })}
		</p>
	</div>

	{#if stats}
		<div class="grid grid-cols-1 gap-4 md:grid-cols-4">
			<div class="rounded-card border border-line bg-raised p-4">
				<p class="text-xs text-fg-muted">{$t('common.total')}</p>
				<p class="mt-1 text-2xl font-semibold text-fg">{stats.total}</p>
			</div>
			<div class="rounded-card border border-line bg-raised p-4">
				<p class="text-xs text-fg-muted">{$t('governanceExt.overallAlignmentRate')}</p>
				<p class="mt-1 text-2xl font-semibold text-success">
					{pct(stats.overall_alignment_rate)}
				</p>
			</div>
			<div class="rounded-card border border-line bg-raised p-4">
				<p class="text-xs text-fg-muted">{$t('governanceExt.recentAlignmentRate')}</p>
				<p class="mt-1 text-2xl font-semibold text-fg">{pct(stats.recent_alignment_rate)}</p>
			</div>
			<div class="rounded-card border border-line bg-raised p-4">
				<p class="text-xs text-fg-muted">{$t('governanceExt.improvementTrend')}</p>
				<p
					class={`mt-1 text-2xl font-semibold ${stats.improvement_trend >= 0 ? 'text-success' : 'text-danger'}`}
				>
					{stats.improvement_trend >= 0 ? '+' : ''}{pct(stats.improvement_trend)}
				</p>
			</div>
		</div>
	{/if}

	<div class="grid grid-cols-1 gap-3 rounded-card border border-line bg-raised p-4 md:grid-cols-4">
		<div>
			<label for="learning-domain" class="mb-1 block text-sm text-fg-muted"
				>{$t('governance.domainLabel')}</label
			>
			<input
				id="learning-domain"
				bind:value={domain}
				type="text"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			/>
		</div>
		<div class="md:col-span-3 flex items-end">
			<button
				type="button"
				onclick={search}
				class="w-full rounded-control rf-filled bg-primary px-3 py-2 text-sm text-on-primary"
			>
				{$t('common.search')}
			</button>
		</div>
	</div>

	<div class="rounded-card border border-line bg-raised p-4">
		{#if loading}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if records.length === 0}
			<p class="text-sm text-fg-muted">{$t('common.noData')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full text-left text-sm">
					<thead>
						<tr class="border-b border-line text-xs text-fg-muted">
							<th class="px-2 py-2">{$t('aiAgents.review')}</th>
							<th class="px-2 py-2">{$t('governance.domainLabel')}</th>
							<th class="px-2 py-2">{$t('governanceExt.alignment')}</th>
							<th class="px-2 py-2">{$t('aiAgents.rating')}</th>
							<th class="px-2 py-2">{$t('common.createdAt')}</th>
							<th class="px-2 py-2">{$t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each records as record}
							<tr class="border-b border-line">
								<td class="px-2 py-2 text-xs">
									<a href={`/proposals/${record.proposal_id}`} class="text-fg underline"
										>{record.review_id}</a
									>
								</td>
								<td class="px-2 py-2 text-xs">{record.domain}</td>
								<td class="px-2 py-2 text-xs">
									<span class={`rounded px-2 py-0.5 ${tone(record.outcome_alignment)}`}
										>{record.outcome_alignment}</span
									>
								</td>
								<td class="px-2 py-2 text-xs">{record.review_rating}</td>
								<td class="px-2 py-2 text-xs text-fg-muted"
									>{new Date(record.created_at).toLocaleString()}</td
								>
								<td class="px-2 py-2 text-xs">
									<button
										type="button"
										class="text-fg underline"
										onclick={() => loadFeedback(record.review_id)}
									>
										{$t('governanceExt.viewFeedback')}
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-line pt-3 text-sm">
			<p class="text-fg-muted">
				{$t('governance.pagination', { values: { total, page: pageNum, pages: totalPages || 1 } })}
			</p>
			<div class="flex gap-2">
				<button
					type="button"
					onclick={() => goPage(pageNum - 1)}
					disabled={pageNum <= 1}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				>
					{$t('governance.prevPage')}
				</button>
				<button
					type="button"
					onclick={() => goPage(pageNum + 1)}
					disabled={totalPages > 0 && pageNum >= totalPages}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
				>
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>

	<div class="rounded-card border border-line bg-raised p-4">
		<div class="mb-3 flex items-center justify-between">
			<h2 class="text-sm font-semibold text-fg">
				{$t('governanceExt.feedbackForReview', { values: { id: feedbackReviewId || '-' } })}
			</h2>
			{#if loadingFeedback}
				<span class="text-xs text-fg-muted">{$t('common.loading')}</span>
			{/if}
		</div>
		{#if feedbackItems.length === 0}
			<p class="text-sm text-fg-muted">{$t('common.noData')}</p>
		{:else}
			<div class="space-y-2">
				{#each feedbackItems as item}
					<div class="rounded border border-line p-3 text-xs">
						<p class="text-fg">{item.lesson_learned || '-'}</p>
						<p class="mt-1 text-fg-muted">{item.follow_up_improvement || '-'}</p>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
