<script lang="ts">
	// Conversion job page body. Polling, the terminal and retryable status tables, the retry
	// confirmation gate, the per-job retry key and the source/target resolution live in
	// `ConversionJobController`; this component renders its snapshot and holds only focus,
	// clipboard and announcement state.
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import Card from '#lib/components/Card.svelte';
	import EmptyState from '#lib/components/EmptyState.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import type { FlowCommandService } from '#lib/flow/command-service.js';
	import {
		ConversionJobController,
		jobErrorKey,
		jobStatusKey,
		jobTargetTypeKey,
		type ConversionJobSnapshot
	} from '#lib/flow/conversion-job.js';
	import { parseConversionWarnings } from '#lib/flow/convert-wizard.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';

	interface Props {
		workspaceId: string | null;
		jobId: string;
		repository: Pick<FlowObjectRepository, 'get'>;
		commands: Pick<FlowCommandService, 'convertStatus' | 'convertRetry'>;
	}

	let { workspaceId, jobId, repository, commands }: Props = $props();

	// Snapshots are plain objects issued by the controller; `$state.raw` keeps them unproxied.
	let snap = $state.raw<ConversionJobSnapshot | null>(null);
	let liveKey = $state('');
	let copyState = $state<{ id: string; ok: boolean } | null>(null);
	let confirmOpen = $state(false);
	let cancelButton = $state<HTMLButtonElement | null>(null);
	let retryButton = $state<HTMLButtonElement | null>(null);

	// The page is keyed by its route, so workspace and job are fixed for this component.
	const controller = new ConversionJobController(
		untrack(() => workspaceId),
		untrack(() => jobId),
		(next) => {
			const previous = snap;
			snap = next;
			announceTransition(previous, next);
		},
		untrack(() => ({ commands, repository }))
	);
	snap = controller.snapshot();

	onMount(() => {
		void controller.poll();
	});
	onDestroy(() => controller.dispose());

	function announceTransition(previous: ConversionJobSnapshot | null, next: ConversionJobSnapshot) {
		if (next.loadState === 'not_found' && previous?.loadState !== 'not_found') {
			liveKey = 'flow.bridge.job.live.notFound';
			return;
		}
		const before = previous?.job?.status;
		const after = next.job?.status;
		if (after === before) return;
		if (after === 'completed') liveKey = 'flow.bridge.job.live.completed';
		else if (after === 'failed') liveKey = 'flow.bridge.job.live.failed';
	}

	async function openRetry() {
		if (!controller.requestRetry()) return;
		confirmOpen = true;
		await tick();
		cancelButton?.focus();
	}

	async function cancelRetry() {
		controller.cancelRetry();
		confirmOpen = false;
		await tick();
		retryButton?.focus();
	}

	async function confirmRetry() {
		confirmOpen = false;
		liveKey = 'flow.bridge.job.live.retrying';
		await controller.confirmRetry();
	}

	async function copyId(id: string) {
		try {
			await navigator.clipboard.writeText(id);
			copyState = { id, ok: true };
		} catch {
			copyState = { id, ok: false };
		}
	}

	const statusClass: Record<string, string> = {
		started: 'border-line bg-warning-soft text-warning',
		completed: 'border-line bg-surface text-fg',
		failed: 'border-line bg-danger-soft text-danger'
	};
	const statusFallbackClass = 'border-line-strong bg-surface text-fg';

	const buttonPrimary =
		'min-h-[44px] min-w-[44px] rounded-control bg-primary rf-filled px-4 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-fg-muted motion-safe:transition-colors';
	const buttonSecondary =
		'min-h-[44px] min-w-[44px] rounded-control border border-fg bg-raised px-4 text-sm font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted disabled:border-line';
	const linkClass =
		'inline-flex min-h-11 items-center break-all text-fg underline-offset-2 underline';
	const dtClass = 'text-sm text-fg-muted';
	const ddClass = 'text-sm text-fg';
	const monoClass = 'break-all font-mono text-xs text-fg';
</script>

<Card>
	<section aria-labelledby="flow-conversion-heading" data-testid="flow-conversion">
		<h1 id="flow-conversion-heading" class="text-lg font-semibold text-fg">
			{$t('flow.bridge.job.heading')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">
			{$t('flow.bridge.job.description')}
		</p>
		<p class="mt-2 text-xs text-fg-muted">
			{$t('flow.bridge.job.jobId')}:
			<span class="break-all font-mono" data-testid="flow-conversion-job-id">{jobId}</span>
		</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-conversion-live">
			{liveKey ? $t(liveKey) : ''}
		</p>

		{#if snap}
			{#if snap.loadState === 'loading'}
				<p class="mt-6 text-sm text-fg-muted" data-testid="flow-conversion-loading">
					{$t('flow.bridge.job.loading')}
				</p>
			{:else if snap.loadState === 'not_found'}
				<div class="mt-6" data-testid="flow-conversion-not-found">
					<EmptyState
						icon="search"
						title={$t('flow.bridge.job.notFoundTitle')}
						description={$t('flow.bridge.job.notFound')}
					/>
				</div>
			{:else if snap.loadState === 'failed' && snap.loadFailure}
				<p
					class="mt-6 text-sm text-danger"
					role="alert"
					data-testid="flow-conversion-load-error"
					data-kind={snap.loadFailure.kind}
				>
					{$t(snap.loadFailure.messageKey)}
				</p>
			{:else if snap.job}
				{@const job = snap.job}
				{@const errorKey = jobErrorKey(job.error, job.error_reason)}
				<div class="mt-6 flex flex-wrap items-center gap-3">
					<span class="text-sm text-fg-muted">{$t('flow.bridge.job.statusLabel')}</span>
					<span
						class="inline-flex min-h-8 items-center rounded-full border px-3 text-sm font-medium {statusClass[
							job.status
						] ?? statusFallbackClass}"
						data-testid="flow-conversion-status"
						data-status={job.status}
					>
						{$t(jobStatusKey(job.status))}
					</span>
					{#if snap.polling && !snap.terminal}
						<span class="text-sm text-fg-muted" data-testid="flow-conversion-polling">
							{$t('flow.bridge.job.polling')}
						</span>
					{/if}
				</div>

				{#if snap.pollFailure}
					<p
						class="mt-3 text-sm text-warning"
						data-testid="flow-conversion-poll-error"
						data-kind={snap.pollFailure.kind}
					>
						{snap.pollFailure.retryable
							? $t('flow.bridge.job.pollFailed')
							: $t(snap.pollFailure.messageKey)}
					</p>
				{/if}

				<dl class="mt-6 grid grid-cols-1 gap-x-6 gap-y-3 md:grid-cols-[minmax(10rem,auto)_1fr]">
					<dt class={dtClass}>{$t('flow.bridge.job.field.source')}</dt>
					<dd class={ddClass} data-testid="flow-conversion-source" data-state={snap.source.state}>
						{#if snap.source.state === 'ready'}
							<a
								class={linkClass}
								href={resolve('/(app)/flow/[objectId]', { objectId: job.source_object_id })}
								data-testid="flow-conversion-source-link"
							>
								{snap.source.title}
							</a>
							<span class="block {monoClass}">{job.source_object_id}</span>
						{:else if snap.source.state === 'loading'}
							<span class="text-fg-muted">{$t('flow.bridge.job.field.sourceLoading')}</span>
							<span class="block {monoClass}">{job.source_object_id}</span>
						{:else}
							<span class={monoClass} data-testid="flow-conversion-source-id"
								>{job.source_object_id}</span
							>
							<span class="block text-xs text-fg-muted">
								{$t('flow.bridge.job.field.sourceHidden')}
							</span>
						{/if}
					</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.targetType')}</dt>
					<dd
						class={ddClass}
						data-testid="flow-conversion-target-type"
						data-target-type={job.target_type}
					>
						{$t(jobTargetTypeKey(job.target_type))}
					</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.sourceFrontier')}</dt>
					<dd class={monoClass} data-testid="flow-conversion-frontier">{job.source_frontier}</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.targetSchemaVersion')}</dt>
					<dd class={ddClass} data-testid="flow-conversion-schema-version">
						{job.target_schema_version}
					</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.lineage')}</dt>
					<dd data-testid="flow-conversion-lineage">
						{#if job.lineage_id}
							<span class={monoClass}>{job.lineage_id}</span>
						{:else}
							<span class="text-sm text-fg-muted">{$t('flow.bridge.job.field.lineageNone')}</span>
						{/if}
					</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.warnings')}</dt>
					<dd data-testid="flow-conversion-warnings" data-count={job.warnings.length}>
						{#if job.warnings.length === 0}
							<span class="text-sm text-fg-muted">{$t('flow.bridge.job.field.warningsNone')}</span>
						{:else}
							<ul class="list-disc space-y-1 pl-5 text-sm">
								{#each parseConversionWarnings(job.warnings) as warning, index (index)}
									<li data-testid="flow-conversion-warning" data-code={warning.code ?? 'unknown'}>
										{$t(warning.messageKey, { values: { field: warning.field } })}
									</li>
								{/each}
							</ul>
						{/if}
					</dd>

					{#if errorKey}
						<dt class={dtClass}>{$t('flow.bridge.job.field.error')}</dt>
						<dd
							class="text-sm text-danger"
							data-testid="flow-conversion-error"
							data-code={job.error}
							data-reason={job.error_reason ?? undefined}
						>
							{$t(errorKey)}
						</dd>
					{/if}
				</dl>

				<section class="mt-6" aria-labelledby="flow-conversion-targets-heading">
					<h2 id="flow-conversion-targets-heading" class="text-base font-semibold text-fg">
						{$t('flow.bridge.job.field.createdTargets')}
					</h2>
					{#if snap.targets.length === 0}
						<p class="mt-2 text-sm text-fg-muted" data-testid="flow-conversion-targets-empty">
							{$t('flow.bridge.job.field.createdTargetsNone')}
						</p>
					{:else}
						<ul class="mt-2 space-y-3" data-testid="flow-conversion-targets">
							{#each snap.targets as target (target.id)}
								<li
									class="rounded-card border border-line p-3"
									data-testid="flow-conversion-target"
									data-target-id={target.id}
									data-state={target.state}
								>
									{#if target.state === 'form'}
										<a
											class={linkClass}
											href={resolve(
												`/(app)/workspace/[workspaceId]/projects/[projectId]/forms?form=${encodeURIComponent(target.id)}`,
												{
													workspaceId: target.workspaceId ?? workspaceId!,
													projectId: target.projectId
												}
											)}
											data-testid="flow-conversion-target-link"
											>{target.title || $t('flow.bridge.job.field.targetUntitled')}</a
										>
										<span class="block {monoClass}">{target.id}</span>
									{:else if target.state === 'record'}
										<a
											class={linkClass}
											href={resolve(
												'/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]',
												{
													workspaceId: target.workspaceId ?? workspaceId!,
													projectId: target.projectId,
													recordId: target.id
												}
											)}
											data-testid="flow-conversion-target-link"
										>
											{target.title || $t('flow.bridge.job.field.targetUntitled')}
										</a>
										<span class="block {monoClass}">{target.id}</span>
									{:else if target.state === 'loading'}
										<span class={monoClass}>{target.id}</span>
										<span class="block text-xs text-fg-muted">
											{$t('flow.bridge.job.field.targetLoading')}
										</span>
									{:else}
										<div class="flex flex-wrap items-center gap-3">
											<span class={monoClass} data-testid="flow-conversion-target-id"
												>{target.id}</span
											>
											<button
												type="button"
												class={buttonSecondary}
												onclick={() => copyId(target.id)}
												data-testid="flow-conversion-target-copy"
											>
												{$t('flow.bridge.job.action.copyId')}
											</button>
										</div>
										<span class="mt-1 block text-xs text-fg-muted">
											{$t('flow.bridge.job.field.targetUnresolved')}
										</span>
										{#if copyState?.id === target.id}
											<span
												class="mt-1 block text-xs {copyState.ok ? 'text-success' : 'text-danger'}"
												role="status"
												aria-live="polite"
											>
												{copyState.ok
													? $t('flow.bridge.job.action.copied')
													: $t('flow.bridge.job.action.copyFailed')}
											</span>
										{/if}
									{/if}
								</li>
							{/each}
						</ul>
					{/if}
				</section>

				{#if snap.canRetry || snap.retrying || snap.retryFailure}
					<div class="mt-6 space-y-3 border-t border-line pt-4">
						{#if snap.retryFailure}
							<p
								class="text-sm text-danger"
								role="alert"
								data-testid="flow-conversion-retry-error"
								data-kind={snap.retryFailure.kind}
								data-reason={snap.retryFailure.reason ?? undefined}
							>
								{$t(snap.retryFailure.messageKey, {
									values: snap.retryFailure.messageValues ?? {}
								})}
							</p>
						{/if}
						<div class="flex flex-wrap gap-3">
							{#if snap.canRetry || snap.retrying}
								<button
									type="button"
									class={buttonPrimary}
									bind:this={retryButton}
									disabled={!snap.canRetry}
									onclick={openRetry}
									data-testid="flow-conversion-retry"
								>
									{snap.retrying
										? $t('flow.bridge.job.action.retrying')
										: $t('flow.bridge.job.action.retry')}
								</button>
							{/if}
							{#if snap.retryFailure && !snap.retryFailure.retryable}
								<a
									class={linkClass}
									href={resolve('/(app)/flow/[objectId]/convert', {
										objectId: job.source_object_id
									})}
									data-testid="flow-conversion-new"
								>
									{$t('flow.bridge.job.action.newConversion')}
								</a>
							{/if}
						</div>
					</div>
				{/if}
			{/if}
		{/if}
	</section>
</Card>

<Modal
	bind:open={confirmOpen}
	title={$t('flow.bridge.job.retry.confirmTitle')}
	onclose={cancelRetry}
>
	<div class="space-y-4" data-testid="flow-conversion-retry-dialog">
		<p class="text-sm text-fg">
			{$t('flow.bridge.job.retry.confirmBody')}
		</p>
		<div class="flex flex-wrap justify-end gap-3">
			<button
				type="button"
				class={buttonSecondary}
				bind:this={cancelButton}
				onclick={cancelRetry}
				data-testid="flow-conversion-retry-cancel"
			>
				{$t('flow.bridge.job.retry.cancel')}
			</button>
			<button
				type="button"
				class={buttonPrimary}
				onclick={confirmRetry}
				data-testid="flow-conversion-retry-confirm"
			>
				{$t('flow.bridge.job.retry.confirm')}
			</button>
		</div>
	</div>
</Modal>
