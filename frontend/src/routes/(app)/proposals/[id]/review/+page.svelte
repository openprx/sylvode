<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { proposalsApi, type Proposal } from '#lib/api/proposals.js';
	import {
		impactReviewsApi,
		type ImpactReviewDetail,
		type ReviewRating,
		type ReviewStatus
	} from '#lib/api/impact-reviews.js';
	import { toast } from '#lib/stores/toast.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { isMissingResult } from '#lib/utils/api-helpers.js';

	const proposalId = $derived(page.params.id || '');

	let loading = $state(true);
	let loadFailure = $state<'missing' | 'failed' | null>(null);
	let loadFailureMessage = $state('');
	let proposal = $state<Proposal | null>(null);
	let impactReviewDetail = $state<ImpactReviewDetail | null>(null);
	let loadingImpactReview = $state(false);
	let impactRating = $state<ReviewRating | ''>('');
	let impactStatus = $state<ReviewStatus>('collecting');
	let impactAchievements = $state('');
	let impactLessons = $state('');
	let impactGoalAchievementsText = $state('[]');
	let impactSaving = $state(false);

	const pageTitle = $derived.by(() => {
		if (proposal?.title?.trim()) {
			return $t('pageTitle.proposalImpactReviewWithTitle', {
				values: { title: proposal.title.trim() }
			});
		}
		return $t('pageTitle.proposalImpactReview');
	});

	onMount(() => {
		void init();
	});

	async function init() {
		loading = true;
		loadFailure = null;
		if (!proposalId) {
			loading = false;
			return;
		}

		const proposalRes = await proposalsApi.get(proposalId);
		if (proposalRes.code !== 0 || !proposalRes.data) {
			loadFailureMessage = proposalRes.message || get(t)('governance.notFound');
			loadFailure = isMissingResult(proposalRes) ? 'missing' : 'failed';
			// A missing proposal is announced in the tip slot; a failed request by the block's own alert.
			if (loadFailure === 'missing') toast.error(loadFailureMessage);
			loading = false;
			return;
		}

		proposal = proposalRes.data.proposal;
		await loadImpactReview();
		loading = false;
	}

	function syncImpactForm() {
		impactRating = impactReviewDetail?.review.rating || '';
		impactStatus = impactReviewDetail?.review.status || 'collecting';
		impactAchievements = impactReviewDetail?.review.achievements || '';
		impactLessons = impactReviewDetail?.review.lessons || '';
		impactGoalAchievementsText = JSON.stringify(
			impactReviewDetail?.review.goal_achievements || [],
			null,
			2
		);
	}

	async function loadImpactReview() {
		if (!proposalId) {
			impactReviewDetail = null;
			return;
		}
		loadingImpactReview = true;
		const res = await impactReviewsApi.getByProposal(proposalId);
		loadingImpactReview = false;
		if (res.code === 0 && res.data) {
			impactReviewDetail = res.data;
			syncImpactForm();
			return;
		}
		impactReviewDetail = null;
	}

	async function createImpactReview() {
		if (!proposalId) return;
		const res = await impactReviewsApi.createForProposal(proposalId);
		if (res.code !== 0) {
			toast.error(res.message || get(t)('impactReview.createFailed'));
			return;
		}
		toast.success(get(t)('impactReview.createSuccess'));
		await loadImpactReview();
	}

	async function saveImpactReview() {
		if (!proposalId || !impactReviewDetail) return;
		let parsedGoalAchievements: unknown = [];
		try {
			parsedGoalAchievements = JSON.parse(impactGoalAchievementsText || '[]');
		} catch {
			toast.error(get(t)('impactReview.goalAchievementsInvalid'));
			return;
		}

		impactSaving = true;
		const res = await impactReviewsApi.updateByProposal(proposalId, {
			status: impactStatus,
			rating: impactRating || undefined,
			goal_achievements: parsedGoalAchievements,
			achievements: impactAchievements.trim() || undefined,
			lessons: impactLessons.trim() || undefined
		});
		impactSaving = false;
		if (res.code !== 0 || !res.data) {
			toast.error(res.message || get(t)('impactReview.updateFailed'));
			return;
		}
		impactReviewDetail = res.data;
		syncImpactForm();
		toast.success(get(t)('impactReview.updateSuccess'));
	}
</script>

<svelte:head>
	<title>{pageTitle}</title>
</svelte:head>

<div class="mx-auto max-w-4xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<div class="mb-2 flex items-center justify-between gap-3">
			<h1 class="text-xl font-semibold text-fg">
				{$t('impactReview.title')}
			</h1>
			<a href={`/proposals/${proposalId}`} class="text-sm text-fg underline"
				>{$t('impactReview.backToProposal')}</a
			>
		</div>
		<p class="text-sm text-fg-muted">{$t('impactReview.detailSubtitle')}</p>
	</div>

	<div class="rounded-card border border-line bg-raised p-5">
		{#if loading}
			<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
		{:else if loadFailure === 'failed'}
			<StateBlock
				kind="loadFailed"
				message={loadFailureMessage}
				action={{ run: () => void init() }}
				data-testid="proposal-review-load-failed"
			/>
		{:else if !proposal}
			<StateBlock
				kind="notFound"
				place={$t('governance.place')}
				action={{ run: () => goto('/proposals') }}
				data-testid="proposal-review-not-found"
			/>
		{:else}
			<div class="mb-3 text-xs text-fg-muted">
				<p>{$t('impactReview.proposalId')}: {proposal.id}</p>
				<p>{$t('governance.createdAtLabel')}: {new Date(proposal.created_at).toLocaleString()}</p>
			</div>

			{#if loadingImpactReview}
				<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
			{:else if !impactReviewDetail}
				<p class="text-sm text-fg-muted">{$t('impactReview.notCreated')}</p>
				{#if proposal.status === 'approved'}
					<button
						type="button"
						onclick={createImpactReview}
						class="mt-3 rounded-control rf-filled bg-primary px-3 py-2 text-sm text-on-primary"
					>
						{$t('impactReview.create')}
					</button>
				{/if}
			{:else}
				<div class="mb-3 grid grid-cols-1 gap-2 text-xs text-fg-muted md:grid-cols-4">
					<p>{$t('impactReview.reviewId')}: {impactReviewDetail.review.id}</p>
					<p>
						{$t('impactReview.scheduledAt')}: {impactReviewDetail.review.scheduled_at
							? new Date(impactReviewDetail.review.scheduled_at).toLocaleString()
							: '-'}
					</p>
					<p>
						{$t('impactReview.conductedAt')}: {impactReviewDetail.review.conducted_at
							? new Date(impactReviewDetail.review.conducted_at).toLocaleString()
							: '-'}
					</p>
					<p>
						{$t('impactReview.trustApplied')}: {impactReviewDetail.review.trust_score_applied
							? $t('impactReview.yes')
							: $t('impactReview.no')}
					</p>
				</div>
				<div class="space-y-3">
					<div class="grid grid-cols-1 gap-3 md:grid-cols-2">
						<div>
							<label for="proposal-review-status" class="mb-1 block text-sm text-fg-muted"
								>{$t('common.status')}</label
							>
							<select
								id="proposal-review-status"
								bind:value={impactStatus}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
							>
								<option value="pending">{$t('impactReview.status.pending')}</option>
								<option value="collecting">{$t('impactReview.status.collecting')}</option>
								<option value="completed">{$t('impactReview.status.completed')}</option>
								<option value="skipped">{$t('impactReview.status.skipped')}</option>
							</select>
						</div>
						<div>
							<label for="proposal-review-rating" class="mb-1 block text-sm text-fg-muted"
								>{$t('impactReview.rating')}</label
							>
							<select
								id="proposal-review-rating"
								bind:value={impactRating}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
							>
								<option value="">{`- ${$t('impactReview.ratingAll')} -`}</option>
								<option value="S">S</option>
								<option value="A">A</option>
								<option value="B">B</option>
								<option value="C">C</option>
								<option value="F">F</option>
							</select>
						</div>
					</div>
					<div>
						<label for="proposal-review-goals" class="mb-1 block text-sm text-fg-muted"
							>{$t('impactReview.goalAchievements')}</label
						>
						<textarea
							id="proposal-review-goals"
							bind:value={impactGoalAchievementsText}
							rows="6"
							class="w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs"
						></textarea>
					</div>
					<div>
						<label for="proposal-review-achievements" class="mb-1 block text-sm text-fg-muted"
							>{$t('impactReview.achievements')}</label
						>
						<textarea
							id="proposal-review-achievements"
							bind:value={impactAchievements}
							rows="4"
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
						></textarea>
					</div>
					<div>
						<label for="proposal-review-lessons" class="mb-1 block text-sm text-fg-muted"
							>{$t('impactReview.lessons')}</label
						>
						<textarea
							id="proposal-review-lessons"
							bind:value={impactLessons}
							rows="4"
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm"
						></textarea>
					</div>
					<div class="flex justify-end">
						<button
							type="button"
							onclick={saveImpactReview}
							class="rounded-control rf-filled bg-primary px-3 py-2 text-sm text-on-primary"
							disabled={impactSaving}
						>
							{impactSaving ? $t('common.saving') : $t('common.save')}
						</button>
					</div>
				</div>
			{/if}
		{/if}
	</div>
</div>
