<script lang="ts">
	// Conversion job page body. Polling, the terminal and retryable status tables, the retry
	// confirmation gate, the per-job retry key and the source/target resolution live in
	// `ConversionJobController`; this component renders its snapshot and holds only focus,
	// clipboard and announcement state.
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import Card from '$lib/components/Card.svelte';
	import EmptyState from '$lib/components/EmptyState.svelte';
	import Modal from '$lib/components/Modal.svelte';
	import type { FlowCommandService } from '$lib/flow/command-service';
	import {
		ConversionJobController,
		jobErrorKey,
		jobStatusKey,
		type ConversionJobSnapshot
	} from '$lib/flow/conversion-job';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';

	interface Props {
		workspaceId: string;
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

	function warningText(warning: unknown): string {
		return typeof warning === 'string' ? warning : JSON.stringify(warning);
	}

	const statusClass: Record<string, string> = {
		started:
			'border-amber-300 bg-amber-50 text-amber-800 dark:border-amber-700 dark:bg-amber-950/40 dark:text-amber-200',
		completed:
			'border-emerald-300 bg-emerald-50 text-emerald-800 dark:border-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-200',
		failed:
			'border-red-300 bg-red-50 text-red-800 dark:border-red-700 dark:bg-red-950/40 dark:text-red-200'
	};
	const statusFallbackClass =
		'border-slate-300 bg-slate-50 text-slate-700 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-200';

	const buttonPrimary =
		'min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900';
	const buttonSecondary =
		'min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800';
	const linkClass =
		'inline-flex min-h-11 items-center break-all text-blue-700 underline-offset-2 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300';
	const dtClass = 'text-sm text-slate-500 dark:text-slate-400';
	const ddClass = 'text-sm text-slate-900 dark:text-slate-100';
	const monoClass = 'break-all font-mono text-xs text-slate-900 dark:text-slate-100';
</script>

<Card>
	<section aria-labelledby="flow-conversion-heading" data-testid="flow-conversion">
		<h1
			id="flow-conversion-heading"
			class="text-lg font-semibold text-slate-900 dark:text-slate-100"
		>
			{$t('flow.bridge.job.heading')}
		</h1>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
			{$t('flow.bridge.job.description')}
		</p>
		<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
			{$t('flow.bridge.job.jobId')}:
			<span class="break-all font-mono" data-testid="flow-conversion-job-id">{jobId}</span>
		</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-conversion-live">
			{liveKey ? $t(liveKey) : ''}
		</p>

		{#if snap}
			{#if snap.loadState === 'loading'}
				<p
					class="mt-6 text-sm text-slate-500 dark:text-slate-400"
					data-testid="flow-conversion-loading"
				>
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
					class="mt-6 text-sm text-red-700 dark:text-red-300"
					role="alert"
					data-testid="flow-conversion-load-error"
					data-kind={snap.loadFailure.kind}
				>
					{$t(snap.loadFailure.messageKey)}
				</p>
			{:else if snap.job}
				{@const job = snap.job}
				{@const errorKey = jobErrorKey(job.error)}
				<div class="mt-6 flex flex-wrap items-center gap-3">
					<span class="text-sm text-slate-500 dark:text-slate-400"
						>{$t('flow.bridge.job.statusLabel')}</span
					>
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
						<span
							class="text-sm text-slate-600 dark:text-slate-300"
							data-testid="flow-conversion-polling"
						>
							{$t('flow.bridge.job.polling')}
						</span>
					{/if}
				</div>

				{#if snap.pollFailure}
					<p
						class="mt-3 text-sm text-amber-800 dark:text-amber-200"
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
								href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
									workspaceId,
									objectId: job.source_object_id
								})}
								data-testid="flow-conversion-source-link"
							>
								{snap.source.title}
							</a>
							<span class="block {monoClass}">{job.source_object_id}</span>
						{:else if snap.source.state === 'loading'}
							<span class="text-slate-500 dark:text-slate-400"
								>{$t('flow.bridge.job.field.sourceLoading')}</span
							>
							<span class="block {monoClass}">{job.source_object_id}</span>
						{:else}
							<span class={monoClass} data-testid="flow-conversion-source-id"
								>{job.source_object_id}</span
							>
							<span class="block text-xs text-slate-500 dark:text-slate-400">
								{$t('flow.bridge.job.field.sourceHidden')}
							</span>
						{/if}
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
							<span class="text-sm text-slate-500 dark:text-slate-400"
								>{$t('flow.bridge.job.field.lineageNone')}</span
							>
						{/if}
					</dd>

					<dt class={dtClass}>{$t('flow.bridge.job.field.warnings')}</dt>
					<dd data-testid="flow-conversion-warnings" data-count={job.warnings.length}>
						{#if job.warnings.length === 0}
							<span class="text-sm text-slate-500 dark:text-slate-400"
								>{$t('flow.bridge.job.field.warningsNone')}</span
							>
						{:else}
							<ul class="list-disc space-y-1 pl-5">
								{#each job.warnings as warning, index (index)}
									<li class={monoClass}>{warningText(warning)}</li>
								{/each}
							</ul>
						{/if}
					</dd>

					{#if errorKey}
						<dt class={dtClass}>{$t('flow.bridge.job.field.error')}</dt>
						<dd class="text-sm text-red-700 dark:text-red-300" data-testid="flow-conversion-error">
							{$t(errorKey)}
						</dd>
					{/if}
				</dl>

				<section class="mt-6" aria-labelledby="flow-conversion-targets-heading">
					<h2
						id="flow-conversion-targets-heading"
						class="text-base font-semibold text-slate-900 dark:text-slate-100"
					>
						{$t('flow.bridge.job.field.createdTargets')}
					</h2>
					{#if snap.targets.length === 0}
						<p
							class="mt-2 text-sm text-slate-500 dark:text-slate-400"
							data-testid="flow-conversion-targets-empty"
						>
							{$t('flow.bridge.job.field.createdTargetsNone')}
						</p>
					{:else}
						<ul class="mt-2 space-y-3" data-testid="flow-conversion-targets">
							{#each snap.targets as target (target.id)}
								<li
									class="rounded-md border border-slate-200 p-3 dark:border-slate-700"
									data-testid="flow-conversion-target"
									data-target-id={target.id}
									data-state={target.state}
								>
									{#if target.state === 'record'}
										<a
											class={linkClass}
											href={resolve(
												'/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]',
												{ workspaceId, projectId: target.projectId, recordId: target.id }
											)}
											data-testid="flow-conversion-target-link"
										>
											{target.title || $t('flow.bridge.job.field.targetUntitled')}
										</a>
										<span class="block {monoClass}">{target.id}</span>
									{:else if target.state === 'loading'}
										<span class={monoClass}>{target.id}</span>
										<span class="block text-xs text-slate-500 dark:text-slate-400">
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
										<span class="mt-1 block text-xs text-slate-500 dark:text-slate-400">
											{$t('flow.bridge.job.field.targetUnresolved')}
										</span>
										{#if copyState?.id === target.id}
											<span
												class="mt-1 block text-xs {copyState.ok
													? 'text-emerald-700 dark:text-emerald-300'
													: 'text-red-700 dark:text-red-300'}"
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
					<div class="mt-6 space-y-3 border-t border-slate-200 pt-4 dark:border-slate-700">
						{#if snap.retryFailure}
							<p
								class="text-sm text-red-700 dark:text-red-300"
								role="alert"
								data-testid="flow-conversion-retry-error"
								data-kind={snap.retryFailure.kind}
							>
								{$t(snap.retryFailure.messageKey)}
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
									href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]/convert', {
										workspaceId,
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
		<p class="text-sm text-slate-700 dark:text-slate-200">
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
