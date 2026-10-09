<script lang="ts">
	// Workspace Flow health (`GET /admin/workspaces/{id}/flow/health`). Presentation only: the
	// polling, pause and stale rules live in `HealthMonitor` (`lib/flow/operations-service.ts`).
	import { t } from 'svelte-i18n';
	import {
		HEALTH_STATUS_KEYS,
		deadLetterAlert,
		deadLetterTotal,
		statusKey,
		type HealthSnapshot
	} from '$lib/flow/operations-service';

	interface Props {
		snapshot: HealthSnapshot;
		onpause: () => void;
		onresume: () => void;
		onrefresh: () => void;
	}

	let { snapshot, onpause, onresume, onrefresh }: Props = $props();

	const health = $derived(snapshot.health);
	const alert = $derived(health ? deadLetterAlert(health) : false);

	function seconds(value: number | null): string {
		return value === null
			? $t('flow.operations.none')
			: $t('flow.operations.seconds', { values: { n: Math.round(value) } });
	}

	function rate(value: number): string {
		return $t('flow.operations.perSecond', { values: { n: Number(value.toFixed(2)) } });
	}

	function time(value: number | null): string {
		return value === null ? '' : new Date(value).toLocaleTimeString();
	}
</script>

<section
	class="rounded-lg border border-slate-200 bg-white p-4 shadow-sm sm:p-6 dark:border-slate-700 dark:bg-slate-900"
	aria-labelledby="flow-ops-health-heading"
	data-testid="flow-ops-health"
>
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div class="flex flex-wrap items-center gap-3">
			<h2
				id="flow-ops-health-heading"
				class="text-lg font-semibold text-slate-900 dark:text-slate-100"
			>
				{$t('flow.operations.health.heading')}
			</h2>
			{#if health}
				<span
					class={`rounded-full px-3 py-1 text-xs font-semibold ${health.status === 'healthy' ? 'bg-emerald-100 text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-300' : 'bg-amber-100 text-amber-900 dark:bg-amber-950/50 dark:text-amber-200'}`}
					data-testid="flow-ops-health-status"
				>
					{$t(statusKey(HEALTH_STATUS_KEYS, health.status))}
				</span>
			{/if}
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				type="button"
				class="min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-slate-700 hover:bg-slate-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
				disabled={snapshot.loading}
				onclick={onrefresh}
			>
				{$t('flow.operations.refresh')}
			</button>
			{#if snapshot.paused}
				<button
					type="button"
					class="min-h-11 rounded-md border border-blue-300 px-4 text-sm font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-blue-700 dark:text-blue-300 dark:hover:bg-blue-950/30"
					onclick={onresume}
					data-testid="flow-ops-health-resume"
				>
					{$t('flow.operations.health.resume')}
				</button>
			{:else}
				<button
					type="button"
					class="min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-slate-700 hover:bg-slate-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
					onclick={onpause}
					data-testid="flow-ops-health-pause"
				>
					{$t('flow.operations.health.pause')}
				</button>
			{/if}
		</div>
	</div>

	<p class="mt-2 text-xs text-slate-500 dark:text-slate-400" role="status" aria-live="polite">
		{#if snapshot.paused}
			{$t('flow.operations.health.paused')}
		{:else}
			{$t('flow.operations.health.autoRefresh')}
		{/if}
		{#if snapshot.updatedAt !== null && !snapshot.stale}
			{$t('flow.operations.health.updatedAt', { values: { time: time(snapshot.updatedAt) } })}
		{/if}
	</p>

	{#if snapshot.stale}
		<p
			class="mt-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-200"
			role="alert"
			data-testid="flow-ops-health-stale"
		>
			{$t('flow.operations.health.stale', { values: { time: time(snapshot.updatedAt) } })}
		</p>
	{:else if !health && snapshot.failure}
		<p
			class="mt-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
			role="alert"
		>
			{$t('flow.operations.health.loadFailed')}
			{$t(snapshot.failure.messageKey)}
		</p>
	{/if}

	{#if health}
		{#if health.status !== 'healthy'}
			<p
				class="mt-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-200"
				data-testid="flow-ops-health-degraded"
			>
				{$t('flow.operations.health.degradedNotice')}
			</p>
		{/if}
		<dl
			class="mt-4 grid grid-cols-1 gap-4 text-sm sm:grid-cols-2 lg:grid-cols-3"
			data-testid="flow-ops-health-metrics"
		>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.connections')}
				</dt>
				<dd
					class="mt-1 font-mono text-slate-900 dark:text-slate-100"
					data-testid="flow-ops-health-connections"
				>
					{health.connections}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.acceptRate')}
				</dt>
				<dd class="mt-1 font-mono text-slate-900 dark:text-slate-100">
					{rate(health.accept_rate)}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.rejectRate')}
				</dt>
				<dd class="mt-1 font-mono text-slate-900 dark:text-slate-100">
					{rate(health.reject_rate)}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.queueDepth')}
				</dt>
				<dd class="mt-1 font-mono text-slate-900 dark:text-slate-100">{health.queue_depth}</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.oldestJobAge')}
				</dt>
				<dd class="mt-1 font-mono text-slate-900 dark:text-slate-100">
					{seconds(health.oldest_job_age)}
				</dd>
			</div>
			<div>
				<dt class="text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.storageBytes')}
				</dt>
				<dd
					class="mt-1 font-mono text-slate-900 dark:text-slate-100"
					data-testid="flow-ops-health-storage"
				>
					{$t('flow.operations.bytes', { values: { n: health.storage_bytes } })}
				</dd>
			</div>
		</dl>

		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<div
				class={`rounded-md border p-4 ${alert ? 'border-red-300 bg-red-50 dark:border-red-800 dark:bg-red-950/30' : 'border-slate-200 bg-slate-50 dark:border-slate-700 dark:bg-slate-800/50'}`}
				data-testid="flow-ops-dead-letter"
				data-alert={alert ? 'true' : 'false'}
			>
				<h3
					class={`text-sm font-semibold ${alert ? 'text-red-800 dark:text-red-200' : 'text-slate-900 dark:text-slate-100'}`}
				>
					{$t('flow.operations.health.deadLetter.heading')}
				</h3>
				<dl class="mt-2 grid grid-cols-2 gap-2 text-sm">
					<dt class="text-slate-600 dark:text-slate-300">
						{$t('flow.operations.health.deadLetter.total')}
					</dt>
					<dd class="font-mono" data-testid="flow-ops-dead-letter-total">
						{deadLetterTotal(health)}
					</dd>
					<dt class="text-slate-600 dark:text-slate-300">
						{$t('flow.operations.health.deadLetter.dispatchFailed')}
					</dt>
					<dd class="font-mono">{health.dead_letter.dispatch_failed}</dd>
					<dt class="text-slate-600 dark:text-slate-300">
						{$t('flow.operations.health.deadLetter.deliveryFailed')}
					</dt>
					<dd class="font-mono">{health.dead_letter.delivery_failed}</dd>
					<dt class="text-slate-600 dark:text-slate-300">
						{$t('flow.operations.health.deadLetter.oldestFailedAge')}
					</dt>
					<dd class="font-mono">{seconds(health.dead_letter.oldest_failed_age)}</dd>
				</dl>
				{#if !alert}
					<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
						{$t('flow.operations.health.deadLetter.clear')}
					</p>
				{/if}
			</div>
			<div
				class="rounded-md border border-slate-200 bg-slate-50 p-4 dark:border-slate-700 dark:bg-slate-800/50"
				data-testid="flow-ops-cancelled"
			>
				<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
					{$t('flow.operations.health.cancelled.heading')}
				</h3>
				<p class="mt-2 font-mono text-sm" data-testid="flow-ops-cancelled-count">
					{health.delivery_cancelled}
				</p>
				<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
					{$t('flow.operations.health.cancelled.note')}
				</p>
			</div>
		</div>
	{/if}
</section>
