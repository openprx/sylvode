<script lang="ts">
	// Workspace package export. Request shape, idempotency, polling and the download checksum
	// check live in `PackageExportController` / `downloadExport`; this component only holds
	// ephemeral form state.
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Card from '$lib/components/Card.svelte';
	import {
		EXPORT_STATUS_KEYS,
		PackageExportController,
		downloadExport,
		exportDownloadUrl,
		exportFileName,
		type ExportJobView,
		type FlowExportJob,
		type WorkspaceProjectOption
	} from '$lib/flow/package-export';
	import type { FlowPackageFailure } from '$lib/flow/package-import';

	interface Props {
		workspaceId: string;
		workspaceLabel: string;
		projects: WorkspaceProjectOption[];
	}

	let { workspaceId, workspaceLabel, projects }: Props = $props();

	let includeHistory = $state(false);
	let projectId = $state('');
	let submitting = $state(false);
	let failure = $state.raw<FlowPackageFailure | null>(null);
	let jobs = $state.raw<ExportJobView[]>([]);
	let downloading = $state<string | null>(null);
	let downloadFailure = $state.raw<FlowPackageFailure | null>(null);
	let liveKey = $state('');
	let liveValues = $state<Record<string, string>>({});

	let controller: PackageExportController | null = null;

	function statusLabel(status: string): string {
		const key = EXPORT_STATUS_KEYS[status];
		return key ? $t(key) : $t('flow.export.status.unknown', { values: { status } });
	}

	function formatBytes(size: number): string {
		if (size < 1024) return $t('flow.package.bytes', { values: { n: String(size) } });
		if (size < 1024 * 1024)
			return $t('flow.package.kib', { values: { n: (size / 1024).toFixed(1) } });
		return $t('flow.package.mib', { values: { n: (size / 1024 / 1024).toFixed(1) } });
	}

	function formatTime(value: string): string {
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}

	function announce(key: string, values: Record<string, string> = {}) {
		liveKey = key;
		liveValues = values;
	}

	onMount(() => {
		controller = new PackageExportController(workspaceId, (next) => {
			const previous = new Map(jobs.map((view) => [view.jobId, view.job?.status]));
			jobs = next;
			for (const view of next) {
				const status = view.job?.status;
				if (status && previous.get(view.jobId) !== status && !view.polling) {
					announce('flow.export.live.finished', { status: statusLabel(status) });
				}
			}
		});
		controller.resume();
	});

	onDestroy(() => controller?.dispose());

	async function startExport() {
		if (!controller || submitting) return;
		submitting = true;
		failure = null;
		announce('flow.export.live.starting');
		const outcome = await controller.submit({
			includeHistory,
			projectId: projectId === '' ? null : projectId
		});
		submitting = false;
		if (outcome.status === 'failed') {
			failure = outcome.failure;
			announce('');
		} else {
			announce('flow.export.live.started');
		}
	}

	async function download(job: FlowExportJob) {
		downloading = job.job_id;
		downloadFailure = null;
		const outcome = await downloadExport(job, exportFileName(workspaceLabel, job.job_id));
		downloading = null;
		if (outcome.status === 'failed') {
			downloadFailure = outcome.failure;
			return;
		}
		const url = URL.createObjectURL(outcome.blob);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = outcome.fileName;
		anchor.rel = 'noopener';
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
		announce('flow.export.live.downloaded', { name: outcome.fileName });
	}
</script>

<Card>
	<section aria-labelledby="flow-export-heading" data-testid="flow-export">
		<h2 id="flow-export-heading" class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.export.heading')}
		</h2>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('flow.export.description')}</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-export-live">
			{liveKey ? $t(liveKey, { values: liveValues }) : ''}
		</p>

		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<div>
				<label
					for="flow-export-project"
					class="block text-sm font-medium text-slate-700 dark:text-slate-300"
				>
					{$t('flow.export.scopeLabel')}
				</label>
				<select
					id="flow-export-project"
					class="mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
					bind:value={projectId}
					disabled={submitting}
				>
					<option value="">{$t('flow.export.scopeWorkspace')}</option>
					{#each projects as project (project.id)}
						<option value={project.id}>{project.label}</option>
					{/each}
				</select>
			</div>
			<label
				class="flex min-h-11 cursor-pointer items-center gap-3 self-end text-sm text-slate-900 dark:text-slate-100"
			>
				<input
					type="checkbox"
					class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
					bind:checked={includeHistory}
					disabled={submitting}
				/>
				<span>{$t('flow.export.includeHistory')}</span>
			</label>
		</div>

		<div class="mt-4 flex flex-col gap-3 sm:flex-row sm:items-center">
			<button
				type="button"
				class="min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900"
				disabled={submitting}
				onclick={startExport}
			>
				{submitting ? $t('flow.export.exporting') : $t('flow.export.start')}
			</button>
		</div>

		{#if failure}
			<div
				class="mt-4 flex flex-col gap-3 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700 sm:flex-row sm:items-center sm:justify-between dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
				role="alert"
				data-testid="flow-export-error"
			>
				<span>{$t(failure.messageKey, { values: failure.values })}</span>
				{#if failure.retryable}
					<button
						type="button"
						class="min-h-11 rounded-md border border-red-300 px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 dark:border-red-700"
						onclick={startExport}
					>
						{$t('flow.package.retry')}
					</button>
				{/if}
			</div>
		{/if}

		{#if downloadFailure}
			<div
				class="mt-4 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
				role="alert"
			>
				{$t(downloadFailure.messageKey, { values: downloadFailure.values })}
			</div>
		{/if}

		<h3 class="mt-6 text-sm font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.export.recentHeading')}
		</h3>
		{#if jobs.length === 0}
			<p class="mt-2 text-sm text-slate-500 dark:text-slate-400">{$t('flow.export.recentEmpty')}</p>
		{:else}
			<ul class="mt-2 space-y-3" data-testid="flow-export-jobs">
				{#each jobs as view (view.jobId)}
					{@const job = view.job}
					<li
						class="rounded-md border border-slate-200 p-4 text-sm dark:border-slate-700"
						data-testid="flow-export-job"
						data-job-id={view.jobId}
					>
						<dl class="grid grid-cols-1 gap-2 md:grid-cols-2">
							<div>
								<dt class="text-slate-500 dark:text-slate-400">{$t('flow.export.jobId')}</dt>
								<dd class="break-all font-mono text-xs text-slate-900 dark:text-slate-100">
									{view.jobId}
								</dd>
							</div>
							<div>
								<dt class="text-slate-500 dark:text-slate-400">{$t('flow.export.status.label')}</dt>
								<dd class="text-slate-900 dark:text-slate-100" data-testid="flow-export-status">
									{job ? statusLabel(job.status) : $t('flow.export.status.loading')}
									{#if view.polling}
										<span class="text-slate-500 dark:text-slate-400">
											{$t('flow.export.polling')}
										</span>
									{/if}
								</dd>
							</div>
							{#if job}
								<div class="md:col-span-2">
									<dt class="text-slate-500 dark:text-slate-400">{$t('flow.export.checksum')}</dt>
									<dd
										class="break-all font-mono text-xs text-slate-900 dark:text-slate-100"
										data-testid="flow-export-checksum"
									>
										{job.checksum}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">{$t('flow.export.size')}</dt>
									<dd class="text-slate-900 dark:text-slate-100">{formatBytes(job.size)}</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">{$t('flow.export.expiresAt')}</dt>
									<dd class="text-slate-900 dark:text-slate-100">{formatTime(job.expires_at)}</dd>
								</div>
							{/if}
						</dl>
						{#if view.failure}
							<p class="mt-2 text-sm text-red-700 dark:text-red-300" role="alert">
								{$t(view.failure.messageKey, { values: view.failure.values })}
							</p>
						{/if}
						{#if exportDownloadUrl(job) && job}
							{@const finished = job}
							<button
								type="button"
								class="mt-3 min-h-11 rounded-md border border-blue-300 px-4 text-sm font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-blue-700 dark:text-blue-300 dark:hover:bg-blue-950/30"
								disabled={downloading !== null}
								onclick={() => void download(finished)}
							>
								{downloading === finished.job_id
									? $t('flow.export.downloading')
									: $t('flow.export.download')}
							</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>
</Card>
