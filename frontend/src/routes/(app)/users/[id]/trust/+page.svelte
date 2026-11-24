<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { trustApi, type Appeal, type UserTrustScore } from '#lib/api/trust.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';

	const userId = $derived(page.params.id || '');

	let loading = $state(false);
	let loadingAppeals = $state(false);
	let projectId = $state('');
	let scores = $state<UserTrustScore[]>([]);
	let appeals = $state<Appeal[]>([]);
	let userType = $state<'human' | 'ai'>('human');

	const globalScore = $derived(scores.find((item) => item.domain === 'global'));
	const domainScores = $derived(
		scores.filter((item) => item.domain !== 'global').sort((a, b) => b.score - a.score)
	);

	const totalDomainScore = $derived(
		domainScores.reduce((sum, item) => sum + Math.max(item.score, 0), 0)
	);

	const timeline = $derived(
		scores
			.slice()
			.sort((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
			.slice(0, 30)
	);

	const relatedAppeals = $derived(
		appeals
			.filter((item) => item.appellant_id === userId)
			.sort((a, b) => new Date(b.created_at).getTime() - new Date(a.created_at).getTime())
	);

	onMount(() => {
		void loadAll();
	});

	async function loadAll() {
		await Promise.all([loadTrust(), loadAppeals()]);
	}

	async function loadTrust() {
		loading = true;
		const res = await trustApi.getUserTrust(userId, projectId.trim() || undefined);
		if (res.code !== 0 || !res.data) {
			toast.error(describeResult(res, get(t)('trustProfile.loadFailed')));
			scores = [];
			loading = false;
			return;
		}
		scores = res.data.scores;
		userType = res.data.user_type;
		loading = false;
	}

	async function loadAppeals() {
		loadingAppeals = true;
		const res = await trustApi.listAppeals({ mine: false });
		if (res.code !== 0 || !res.data) {
			loadingAppeals = false;
			return;
		}
		appeals = res.data.items;
		loadingAppeals = false;
	}

	function levelTone(level: string): string {
		switch (level) {
			case 'observer':
				return 'bg-surface text-fg';
			case 'advisor':
				return 'bg-surface text-fg';
			case 'voter':
				return 'bg-surface text-fg';
			case 'vetoer':
				return 'bg-warning-soft text-warning';
			case 'autonomous':
				return 'bg-surface text-fg';
			default:
				return 'bg-surface text-fg';
		}
	}

	function appealTone(status: string): string {
		switch (status) {
			case 'accepted':
				return 'bg-surface text-fg';
			case 'rejected':
				return 'bg-danger-soft text-danger';
			default:
				return 'bg-warning-soft text-warning';
		}
	}

	function barWidth(score: number): string {
		if (totalDomainScore <= 0) {
			return '0%';
		}
		const pct = Math.max(0, Math.min(100, (score / totalDomainScore) * 100));
		return `${pct.toFixed(1)}%`;
	}

	function levelLabel(level: string): string {
		return get(t)(`trustCommon.level.${level}`) || level;
	}

	function appealStatusLabel(status: Appeal['status']): string {
		return get(t)(`appeals.status.${status}`) || status;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.userTrust', { values: { id: userId } })}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-4">
		<div class="mb-4 flex flex-wrap items-end justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-fg">
					{$t('trustProfile.title')}
				</h1>
				<p class="text-sm text-fg-muted">
					{$t('trustProfile.subtitle', { values: { id: userId } })}
				</p>
			</div>
			<div class="flex gap-2">
				<input
					type="text"
					bind:value={projectId}
					placeholder={$t('trustProfile.projectIdPlaceholder')}
					class="rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
				<Button variant="primary" onclick={loadAll} loading={loading || loadingAppeals}
					>{$t('common.search')}</Button
				>
			</div>
		</div>

		{#if loading}
			<div class="py-8 text-center text-fg-muted">{$t('common.loading')}</div>
		{:else if scores.length === 0}
			<div class="py-8 text-center text-fg-muted">{$t('trustProfile.empty')}</div>
		{:else}
			<div class="grid grid-cols-1 gap-4 lg:grid-cols-3">
				<div class="rounded-card border border-line p-4 lg:col-span-1">
					<div class="text-xs uppercase tracking-wide text-fg-muted">
						{$t('trustProfile.globalSummary')}
					</div>
					<div class="mt-2 text-sm text-fg-muted">
						{$t('trustProfile.userType')}: {userType === 'ai'
							? $t('trustBoard.typeAi')
							: $t('trustBoard.typeHuman')}
					</div>
					<div class="mt-3 text-3xl font-semibold text-fg">
						{globalScore?.score ?? 0}
					</div>
					<div class="mt-1 text-sm text-fg-muted">
						{$t('trustProfile.weightLabel')}: {(globalScore?.vote_weight ?? 1).toFixed(2)}x
					</div>
					<div class="mt-2">
						<span
							class={`rounded px-2 py-0.5 text-xs font-medium ${levelTone(globalScore?.level ?? 'observer')}`}
							>{levelLabel(globalScore?.level ?? 'observer')}</span
						>
					</div>
				</div>

				<div class="rounded-card border border-line p-4 lg:col-span-2">
					<div class="mb-3 text-xs uppercase tracking-wide text-fg-muted">
						{$t('trustProfile.domainDistribution')}
					</div>
					{#if domainScores.length === 0}
						<div class="text-sm text-fg-muted">{$t('trustProfile.noDomainData')}</div>
					{:else}
						<div class="space-y-2">
							{#each domainScores as item (item.project_id + item.domain)}
								<div>
									<div class="mb-1 flex items-center justify-between text-xs text-fg-muted">
										<span>{item.domain}</span>
										<span>{item.score}</span>
									</div>
									<div class="h-2 rounded bg-surface">
										<div
											class="h-2 rounded bg-chart-1"
											style={`width:${barWidth(item.score)}`}
										></div>
									</div>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>

			<div class="mt-4 grid grid-cols-1 gap-4 lg:grid-cols-2">
				<div class="rounded-card border border-line p-4">
					<h2 class="text-sm font-semibold text-fg">
						{$t('trustProfile.historyTitle')}
					</h2>
					<div class="mt-3 space-y-2">
						{#each timeline as item (item.project_id + item.domain + item.updated_at)}
							<div class="rounded border border-line px-3 py-2 text-sm">
								<div class="flex items-center justify-between gap-2">
									<div class="font-medium text-fg">{item.domain}</div>
									<div class="text-xs text-fg-muted">
										{new Date(item.updated_at).toLocaleString()}
									</div>
								</div>
								<div class="mt-1 text-xs text-fg-muted">
									{$t('trustProfile.historyMeta', {
										values: {
											score: item.score,
											level: levelLabel(item.level),
											weight: item.vote_weight.toFixed(2)
										}
									})}
								</div>
							</div>
						{/each}
					</div>
				</div>

				<div class="rounded-card border border-line p-4">
					<h2 class="text-sm font-semibold text-fg">
						{$t('trustProfile.changeLogTitle')}
					</h2>
					{#if loadingAppeals}
						<div class="mt-3 text-sm text-fg-muted">{$t('common.loading')}</div>
					{:else if relatedAppeals.length === 0}
						<div class="mt-3 text-sm text-fg-muted">{$t('trustProfile.noChangeLog')}</div>
					{:else}
						<div class="mt-3 space-y-2">
							{#each relatedAppeals as item (item.id)}
								<div class="rounded border border-line px-3 py-2 text-sm">
									<div class="flex items-center justify-between gap-2">
										<div class="font-medium text-fg">
											{$t('trustProfile.appealWithLog', { values: { id: item.log_id } })}
										</div>
										<span class={`rounded px-2 py-0.5 text-xs ${appealTone(item.status)}`}
											>{appealStatusLabel(item.status)}</span
										>
									</div>
									<p class="mt-1 text-xs text-fg-muted">{item.reason}</p>
									<p class="mt-1 text-xs text-fg-muted">
										{new Date(item.created_at).toLocaleString()}
									</p>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		{/if}
	</div>
</div>
