<script lang="ts">
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import {
		governanceExtApi,
		type ProposalChain,
		type TimelineEvent
	} from '#lib/api/governance-ext.js';
	import { toast } from '#lib/stores/toast.js';

	const proposalId = $derived(page.params.id || '');

	let loading = $state(true);
	let data = $state<ProposalChain | null>(null);
	let events = $state<TimelineEvent[]>([]);
	let noLinkedIssues = $state(false);

	const pageTitle = $derived.by(() => {
		if (data?.proposal.title) {
			return $t('pageTitle.proposalChainWithTitle', { values: { title: data.proposal.title } });
		}
		return $t('pageTitle.proposalChain');
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		noLinkedIssues = false;
		const [chainRes, timelineRes] = await Promise.all([
			governanceExtApi.getProposalChain(proposalId),
			governanceExtApi.getProposalTimeline(proposalId)
		]);
		if (chainRes.code !== 0 || !chainRes.data) {
			const message = (chainRes.message || '').toLowerCase();
			if (message.includes('proposal has no linked issues')) {
				noLinkedIssues = true;
				loading = false;
				return;
			}
			toast.error(chainRes.message || get(t)('governanceExt.loadFailed'));
			loading = false;
			return;
		}

		data = chainRes.data;
		noLinkedIssues = chainRes.data.issues.length === 0;
		events =
			timelineRes.code === 0 && timelineRes.data
				? timelineRes.data.events
				: [...chainRes.data.timeline].sort(
						(a, b) => +new Date(a.timestamp) - +new Date(b.timestamp)
					);
		loading = false;
	}

	function eventTone(eventType: string): string {
		if (eventType.includes('decision_approved')) return 'bg-surface text-fg';
		if (eventType.includes('decision_rejected') || eventType.includes('decision_vetoed'))
			return 'bg-danger-soft text-danger';
		if (eventType.includes('review')) return 'bg-surface text-fg';
		if (eventType.includes('vote')) return 'bg-warning-soft text-warning';
		return 'bg-surface text-fg';
	}
</script>

<svelte:head>
	<title>{pageTitle}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-5">
		<h1 class="text-xl font-semibold text-fg">
			{$t('governanceExt.chainTitle')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">{$t('governanceExt.chainSubtitle')}</p>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
			{$t('common.loading')}
		</div>
	{:else if noLinkedIssues}
		<div class="rounded-card border border-line bg-raised p-6">
			<p class="text-sm text-fg-muted">
				{$t('governanceExt.chainEmptyNoLinkedIssues')}
			</p>
			<div class="mt-4">
				<a
					href={resolve('/(app)/proposals/[id]', { id: proposalId })}
					class="inline-flex rounded-control border border-fg bg-raised px-3 py-2 text-xs hover:bg-surface"
				>
					{$t('governanceExt.backToProposal')}
				</a>
			</div>
		</div>
	{:else if data}
		<div class="rounded-card border border-line bg-raised p-5">
			<div class="mb-4 flex items-center justify-between gap-3">
				<div>
					<h2 class="text-lg font-semibold text-fg">
						{data.proposal.title}
					</h2>
					<p class="mt-1 text-xs text-fg-muted">{data.proposal.id}</p>
				</div>
				<a
					href={resolve('/(app)/proposals/[id]', { id: data.proposal.id })}
					class="rounded-control border border-fg bg-raised px-3 py-2 text-xs hover:bg-surface"
				>
					{$t('governanceExt.backToProposal')}
				</a>
			</div>

			<div class="flex flex-wrap items-center gap-2 text-xs">
				<div class="rounded border bg-surface px-3 py-2 text-fg">
					{data.proposal.status}
				</div>
				<span class="text-fg-muted">→</span>
				<div class="rounded border border-line bg-surface px-3 py-2 text-fg">
					{$t('governanceExt.graphIssueCount', { values: { count: data.issues.length } })}
				</div>
				<span class="text-fg-muted">→</span>
				<div class="rounded border bg-warning-soft px-3 py-2 text-warning">
					{data.decision
						? $t('governanceExt.graphDecision', { values: { result: data.decision.result } })
						: $t('governanceExt.graphNoDecision')}
				</div>
				<span class="text-fg-muted">→</span>
				<div class="rounded border bg-surface px-3 py-2 text-fg">
					{data.impact_review
						? $t('governanceExt.graphReview', { values: { status: data.impact_review.status } })
						: $t('governanceExt.graphNoReview')}
				</div>
			</div>
		</div>

		<div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
			<div class="space-y-4 lg:col-span-2">
				<div class="rounded-card border border-line bg-raised p-5">
					<h3 class="mb-3 text-sm font-semibold text-fg">
						{$t('governanceExt.timeline')}
					</h3>
					<div class="space-y-2">
						{#if events.length === 0}
							<p class="text-sm text-fg-muted">{$t('common.noData')}</p>
						{:else}
							{#each events as event, index (index)}
								<div class="flex gap-2 rounded-control border border-line p-3">
									<span class={`rounded px-2 py-0.5 text-xs ${eventTone(event.event_type)}`}
										>{event.event_type}</span
									>
									<div class="min-w-0 flex-1 text-xs">
										<p class="text-fg">{event.description}</p>
										<p class="mt-1 text-fg-muted">
											{new Date(event.timestamp).toLocaleString()} · {event.actor || '-'}
										</p>
									</div>
								</div>
							{/each}
						{/if}
					</div>
				</div>
			</div>

			<div class="space-y-4">
				<div class="rounded-card border border-line bg-raised p-4">
					<h3 class="mb-2 text-sm font-semibold text-fg">
						{$t('governanceExt.votes')}
					</h3>
					<p class="text-xs text-fg-muted">
						{$t('governanceExt.voteCount', { values: { count: data.votes.length } })}
					</p>
					<div class="mt-2 max-h-72 space-y-2 overflow-auto">
						{#each data.votes as vote, index (index)}
							<div class="rounded border border-line px-2 py-1 text-xs">
								<p>{vote.voter_id} · {vote.choice}</p>
								<p class="text-fg-muted">{new Date(vote.voted_at).toLocaleString()}</p>
							</div>
						{/each}
					</div>
				</div>

				<div class="rounded-card border border-line bg-raised p-4">
					<h3 class="mb-2 text-sm font-semibold text-fg">
						{$t('governanceExt.feedbackProposals')}
					</h3>
					{#if data.feedback_proposals.length === 0}
						<p class="text-xs text-fg-muted">{$t('common.noData')}</p>
					{:else}
						<div class="space-y-2">
							{#each data.feedback_proposals as feedback (feedback.proposal_id)}
								<a
									href={resolve('/(app)/proposals/[id]', { id: feedback.proposal_id })}
									class="block rounded border border-line px-2 py-1 text-xs hover:bg-surface"
								>
									<p class="font-medium text-fg">{feedback.title}</p>
									<p class="text-fg-muted">{feedback.link_type}</p>
								</a>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>
