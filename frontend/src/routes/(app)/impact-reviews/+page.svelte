<script lang="ts">
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		impactReviewsApi,
		type ImpactReview,
		type ReviewRating,
		type ReviewStatus
	} from '#lib/api/impact-reviews.js';
	import { toast } from '#lib/stores/toast.js';
	import { projectOptionsStore } from '#lib/stores/project-options.js';
	import { groupProjectOptionsByWorkspace } from '#lib/utils/project-options.js';

	let loading = $state(true);
	let reviews = $state<ImpactReview[]>([]);
	const projects = $derived($projectOptionsStore.items);
	const groupedProjects = $derived.by(() => groupProjectOptionsByWorkspace(projects));
	let selectedProjectId = $state('');
	let status = $state<'all' | ReviewStatus>('all');
	let rating = $state<'all' | ReviewRating>('all');
	let page = $state(1);
	let perPage = $state(20);
	let total = $state(0);
	let totalPages = $state(0);

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		await projectOptionsStore.ensureLoaded();
		if (!selectedProjectId && projects.length > 0) {
			selectedProjectId = projects[0].id;
		}
		await loadReviews();
		loading = false;
	}

	async function loadReviews() {
		const res = await impactReviewsApi.list({
			project_id: selectedProjectId || undefined,
			status: status === 'all' ? undefined : status,
			rating: rating === 'all' ? undefined : rating,
			page,
			per_page: perPage
		});
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('impactReview.loadListFailed'));
			reviews = [];
			total = 0;
			totalPages = 0;
			return;
		}
		reviews = res.data.items ?? [];
		total = res.data.total;
		totalPages = res.data.total_pages;
	}

	async function onSearch() {
		page = 1;
		await loadReviews();
	}

	async function prevPage() {
		if (page <= 1) return;
		page -= 1;
		await loadReviews();
	}

	async function nextPage() {
		if (totalPages > 0 && page >= totalPages) return;
		page += 1;
		await loadReviews();
	}

	function statusTone(reviewStatus: ReviewStatus): string {
		switch (reviewStatus) {
			case 'pending':
				return 'bg-surface text-fg';
			case 'collecting':
				return 'bg-surface text-fg';
			case 'completed':
				return 'bg-surface text-fg';
			case 'skipped':
				return 'bg-warning-soft text-warning';
		}
	}
	function ratingTone(reviewRating?: ReviewRating): string {
		switch (reviewRating) {
			case 'S':
				return 'bg-surface text-fg';
			case 'A':
				return 'bg-surface text-fg';
			case 'B':
				return 'bg-surface text-fg';
			case 'C':
				return 'bg-warning-soft text-warning';
			case 'F':
				return 'bg-danger-soft text-danger';
			default:
				return 'bg-surface text-fg-muted';
		}
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.impactReviews')}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<h1 class="text-xl font-semibold text-fg">
			{$t('impactReview.listTitle')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">{$t('impactReview.listSubtitle')}</p>
	</div>

	<div class="grid grid-cols-1 gap-3 rounded-card border border-line bg-raised p-4 md:grid-cols-4">
		<div>
			<label for="impact-review-project" class="mb-1 block text-sm text-fg-muted"
				>{$t('impactReview.project')}</label
			>
			<select
				id="impact-review-project"
				bind:value={selectedProjectId}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<option value="">{`- ${$t('impactReview.allProjects')} -`}</option>
				{#each groupedProjects as group, index (index)}
					<optgroup label={group.workspaceName}>
						{#each group.items as project (project.id)}
							<option value={project.id}>{project.name}</option>
						{/each}
					</optgroup>
				{/each}
			</select>
		</div>
		<div>
			<label for="impact-review-status" class="mb-1 block text-sm text-fg-muted"
				>{$t('common.status')}</label
			>
			<select
				id="impact-review-status"
				bind:value={status}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<option value="all">{$t('impactReview.status.all')}</option>
				<option value="pending">{$t('impactReview.status.pending')}</option>
				<option value="collecting">{$t('impactReview.status.collecting')}</option>
				<option value="completed">{$t('impactReview.status.completed')}</option>
				<option value="skipped">{$t('impactReview.status.skipped')}</option>
			</select>
		</div>
		<div>
			<label for="impact-review-rating" class="mb-1 block text-sm text-fg-muted"
				>{$t('impactReview.rating')}</label
			>
			<select
				id="impact-review-rating"
				bind:value={rating}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<option value="all">{$t('impactReview.ratingAll')}</option>
				<option value="S">S</option>
				<option value="A">A</option>
				<option value="B">B</option>
				<option value="C">C</option>
				<option value="F">F</option>
			</select>
		</div>
		<div class="flex items-end">
			<button
				type="button"
				onclick={onSearch}
				class="w-full rounded-control rf-filled bg-primary px-3 py-2 text-sm text-on-primary"
			>
				{$t('common.search')}
			</button>
		</div>
	</div>

	<div class="rounded-card border border-line bg-raised p-4">
		{#if loading}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if reviews.length === 0}
			<p class="text-sm text-fg-muted">{$t('impactReview.empty')}</p>
		{:else}
			<div class="space-y-2">
				{#each reviews as review (review.id)}
					<div class="rounded-control border border-line p-3">
						<div class="mb-2 flex flex-wrap items-center gap-2">
							<a
								href={resolve('/(app)/proposals/[id]', { id: review.proposal_id })}
								class="text-sm font-medium text-fg underline"
							>
								{review.proposal_id}
							</a>
							<a
								href={resolve('/(app)/proposals/[id]/review', { id: review.proposal_id })}
								class="text-xs text-fg-muted hover:underline"
							>
								{$t('impactReview.detailLink')}
							</a>
							<span class={`rounded px-2 py-0.5 text-xs ${statusTone(review.status)}`}>
								{$t(`impactReview.status.${review.status}`)}
							</span>
							<span class={`rounded px-2 py-0.5 text-xs ${ratingTone(review.rating)}`}>
								{review.rating || '-'}
							</span>
						</div>
						<div class="grid grid-cols-1 gap-2 text-xs text-fg-muted md:grid-cols-4">
							<p>{$t('impactReview.reviewId')}: {review.id}</p>
							<p>
								{$t('impactReview.scheduledAt')}: {review.scheduled_at
									? new Date(review.scheduled_at).toLocaleString()
									: '-'}
							</p>
							<p>
								{$t('impactReview.conductedAt')}: {review.conducted_at
									? new Date(review.conducted_at).toLocaleString()
									: '-'}
							</p>
							<p>
								{$t('impactReview.trustApplied')}: {review.trust_score_applied
									? $t('impactReview.yes')
									: $t('impactReview.no')}
							</p>
						</div>
					</div>
				{/each}
			</div>
		{/if}

		<div class="mt-4 flex items-center justify-between border-t border-line pt-3 text-sm">
			<p class="text-fg-muted">
				{$t('governance.pagination', { values: { total, page, pages: totalPages || 1 } })}
			</p>
			<div class="flex gap-2">
				<button
					type="button"
					onclick={prevPage}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface"
					disabled={page <= 1}
				>
					{$t('governance.prevPage')}
				</button>
				<button
					type="button"
					onclick={nextPage}
					class="rounded border border-fg bg-raised px-3 py-1.5 text-xs hover:bg-surface"
					disabled={totalPages > 0 && page >= totalPages}
				>
					{$t('governance.nextPage')}
				</button>
			</div>
		</div>
	</div>
</div>
