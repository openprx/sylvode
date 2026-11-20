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
	} from '#lib/flow/operations-service.js';

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
	class="rounded-card border border-line bg-raised p-4 sm:p-6"
	aria-labelledby="flow-ops-health-heading"
	data-testid="flow-ops-health"
>
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div class="flex flex-wrap items-center gap-3">
			<h2 id="flow-ops-health-heading" class="text-lg font-semibold text-fg">
				{$t('flow.operations.health.heading')}
			</h2>
			{#if health}
				<span
					class={`rounded-full px-3 py-1 text-xs font-semibold ${health.status === 'healthy' ? 'bg-surface text-fg' : 'bg-warning-soft text-warning'}`}
					data-testid="flow-ops-health-status"
				>
					{$t(statusKey(HEALTH_STATUS_KEYS, health.status))}
				</span>
			{/if}
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				type="button"
				class="min-h-11 rounded-control border border-fg bg-raised px-4 text-sm font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-control disabled:border-line"
				disabled={snapshot.loading}
				onclick={onrefresh}
			>
				{$t('flow.operations.refresh')}
			</button>
			{#if snapshot.paused}
				<button
					type="button"
					class="min-h-11 rounded-control border border-fg px-4 text-sm font-medium text-fg hover:bg-surface"
					onclick={onresume}
					data-testid="flow-ops-health-resume"
				>
					{$t('flow.operations.health.resume')}
				</button>
			{:else}
				<button
					type="button"
					class="min-h-11 rounded-control border border-fg bg-raised px-4 text-sm font-medium text-fg hover:bg-surface"
					onclick={onpause}
					data-testid="flow-ops-health-pause"
				>
					{$t('flow.operations.health.pause')}
				</button>
			{/if}
		</div>
	</div>

	<p class="mt-2 text-xs text-fg-muted" role="status" aria-live="polite">
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
			class="mt-3 rounded-card border border-line bg-warning-soft p-3 text-sm text-warning"
			role="alert"
			data-testid="flow-ops-health-stale"
		>
			{$t('flow.operations.health.stale', { values: { time: time(snapshot.updatedAt) } })}
		</p>
	{:else if !health && snapshot.failure}
		<p
			class="mt-3 rounded-card border border-line bg-danger-soft p-3 text-sm text-danger"
			role="alert"
		>
			{$t('flow.operations.health.loadFailed')}
			{$t(snapshot.failure.messageKey)}
		</p>
	{/if}

	{#if health}
		{#if health.status !== 'healthy'}
			<p
				class="mt-3 rounded-card border border-line bg-warning-soft p-3 text-sm text-warning"
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
				<dt class="text-fg-muted">
					{$t('flow.operations.health.connections')}
				</dt>
				<dd class="mt-1 font-mono text-fg" data-testid="flow-ops-health-connections">
					{health.connections}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.operations.health.acceptRate')}
				</dt>
				<dd class="mt-1 font-mono text-fg">
					{rate(health.accept_rate)}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.operations.health.rejectRate')}
				</dt>
				<dd class="mt-1 font-mono text-fg">
					{rate(health.reject_rate)}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.operations.health.queueDepth')}
				</dt>
				<dd class="mt-1 font-mono text-fg">{health.queue_depth}</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.operations.health.oldestJobAge')}
				</dt>
				<dd class="mt-1 font-mono text-fg">
					{seconds(health.oldest_job_age)}
				</dd>
			</div>
			<div>
				<dt class="text-fg-muted">
					{$t('flow.operations.health.storageBytes')}
				</dt>
				<dd class="mt-1 font-mono text-fg" data-testid="flow-ops-health-storage">
					{$t('flow.operations.bytes', { values: { n: health.storage_bytes } })}
				</dd>
			</div>
		</dl>

		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<div
				class={`rounded-card border p-4 ${alert ? 'border-line bg-danger-soft' : 'border-line bg-nav'}`}
				data-testid="flow-ops-dead-letter"
				data-alert={alert ? 'true' : 'false'}
			>
				<h3 class={`text-sm font-semibold ${alert ? 'text-danger' : 'text-fg'}`}>
					{$t('flow.operations.health.deadLetter.heading')}
				</h3>
				<dl class="mt-2 grid grid-cols-2 gap-2 text-sm">
					<dt class="text-fg-muted">
						{$t('flow.operations.health.deadLetter.total')}
					</dt>
					<dd class="font-mono" data-testid="flow-ops-dead-letter-total">
						{deadLetterTotal(health)}
					</dd>
					<dt class="text-fg-muted">
						{$t('flow.operations.health.deadLetter.dispatchFailed')}
					</dt>
					<dd class="font-mono">{health.dead_letter.dispatch_failed}</dd>
					<dt class="text-fg-muted">
						{$t('flow.operations.health.deadLetter.deliveryFailed')}
					</dt>
					<dd class="font-mono">{health.dead_letter.delivery_failed}</dd>
					<dt class="text-fg-muted">
						{$t('flow.operations.health.deadLetter.oldestFailedAge')}
					</dt>
					<dd class="font-mono">{seconds(health.dead_letter.oldest_failed_age)}</dd>
				</dl>
				{#if !alert}
					<p class="mt-2 text-xs text-fg-muted">
						{$t('flow.operations.health.deadLetter.clear')}
					</p>
				{/if}
			</div>
			<div class="rounded-card border border-line bg-surface p-4" data-testid="flow-ops-cancelled">
				<h3 class="text-sm font-semibold text-fg">
					{$t('flow.operations.health.cancelled.heading')}
				</h3>
				<p class="mt-2 font-mono text-sm" data-testid="flow-ops-cancelled-count">
					{health.delivery_cancelled}
				</p>
				<p class="mt-2 text-xs text-fg-muted">
					{$t('flow.operations.health.cancelled.note')}
				</p>
			</div>
		</div>
	{/if}
</section>
