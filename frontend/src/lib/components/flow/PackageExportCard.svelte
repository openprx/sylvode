<script lang="ts">
	// Workspace package export. Request shape, idempotency, polling and the download checksum
	// check live in `PackageExportController` / `downloadExport`; this component only holds
	// ephemeral form state.
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Card from '#lib/components/Card.svelte';
	import PackageExportWarnings from './PackageExportWarnings.svelte';
	import {
		EXPORT_STATUS_KEYS,
		PackageExportController,
		downloadExport,
		exportDownloadUrl,
		exportFileName,
		type ExportJobView,
		type FlowExportJob,
		type WorkspaceProjectOption
	} from '#lib/flow/package-export.js';
	import type { FlowPackageObjectType } from '#lib/api/flow.js';
	import type { FlowPackageFailure } from '#lib/flow/package-import.js';

	interface Props {
		scopeId: string;
		workspaceLabel: string;
		projects: WorkspaceProjectOption[];
		allowHistory?: boolean;
	}

	let { scopeId, workspaceLabel, projects, allowHistory = true }: Props = $props();

	let includeHistory = $state(false);
	$effect(() => {
		if (!allowHistory) includeHistory = false;
	});
	let projectId = $state('');
	let objectTypes = $state<FlowPackageObjectType[]>(['page', 'navigator']);
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

	function formatBytes(value: number | null): string {
		if (typeof value !== 'number' || !Number.isFinite(value)) return '—';
		const size = value;
		if (size < 1024) return $t('flow.package.bytes', { values: { n: String(size) } });
		if (size < 1024 * 1024)
			return $t('flow.package.kib', { values: { n: (size / 1024).toFixed(1) } });
		return $t('flow.package.mib', { values: { n: (size / 1024 / 1024).toFixed(1) } });
	}

	function formatTime(value: string | null): string {
		if (!value) return '—';
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
	}

	function announce(key: string, values: Record<string, string> = {}) {
		liveKey = key;
		liveValues = values;
	}

	onMount(() => {
		controller = new PackageExportController(scopeId, (next) => {
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
		if (!controller || submitting || objectTypes.length === 0) return;
		submitting = true;
		failure = null;
		announce('flow.export.live.starting');
		const outcome = await controller.submit({
			includeHistory: allowHistory && includeHistory,
			objectTypes,
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
		<h2 id="flow-export-heading" class="text-lg font-semibold text-fg">
			{$t('flow.export.heading')}
		</h2>
		<p class="mt-1 text-sm text-fg-muted">{$t('flow.export.description')}</p>
		<p
			class="mt-3 rounded-card border border-line bg-warning-soft p-3 text-sm text-fg"
			data-testid="flow-package-history-notice"
		>
			{$t('flow.package.snapshotHistoryNotice')}
		</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-export-live">
			{liveKey ? $t(liveKey, { values: liveValues }) : ''}
		</p>

		<fieldset class="mt-4 space-y-2" disabled={submitting}>
			<legend class="text-sm font-medium text-fg"
				>{$t('flow.package.kindsLabel')}</legend
			>
			<p class="text-sm text-fg-muted">{$t('flow.package.kindsNotice')}</p>
			<div class="flex flex-wrap gap-4">
				<label class="flex min-h-11 items-center gap-2"
					><input type="checkbox" bind:group={objectTypes} value="page" />{$t(
						'flow.package.kindPage'
					)}</label
				>
				<label class="flex min-h-11 items-center gap-2"
					><input type="checkbox" bind:group={objectTypes} value="navigator" />{$t(
						'flow.package.kindNavigator'
					)}</label
				>
			</div>
			{#if objectTypes.length === 0}<p class="text-sm text-danger" role="alert">
					{$t('flow.package.kindRequired')}
				</p>{/if}
		</fieldset>
		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<div>
				<label
					for="flow-export-project"
					class="block text-sm font-medium text-fg"
				>
					{$t('flow.export.scopeLabel')}
				</label>
				<select
					id="flow-export-project"
					class="mt-1 block min-h-11 w-full rounded-control border border-input bg-field px-3 text-sm"
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
				class="flex min-h-11 cursor-pointer items-center gap-3 self-end text-sm text-fg"
			>
				<input
					type="checkbox"
					class="h-5 w-5"
					bind:checked={includeHistory}
					disabled={submitting || !allowHistory}
				/>
				<span>{$t('flow.export.includeHistory')}</span>
			</label>
		</div>
		{#if !allowHistory}<p class="mt-2 text-xs text-fg-muted">
				{$t('flow.personal.packageHistoryUnavailable')}
			</p>{/if}

		<div class="mt-4 flex flex-col gap-3 sm:flex-row sm:items-center">
			<button
				type="button"
				class="min-h-11 rounded-control bg-primary rf-filled px-4 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-control motion-safe:transition-colors"
				disabled={submitting || objectTypes.length === 0}
				onclick={startExport}
			>
				{submitting ? $t('flow.export.exporting') : $t('flow.export.start')}
			</button>
		</div>

		{#if failure}
			<div
				class="mt-4 flex flex-col gap-3 rounded-card border border-line bg-danger-soft p-4 text-sm text-danger sm:flex-row sm:items-center sm:justify-between"
				role="alert"
				data-testid="flow-export-error"
			>
				<span>{$t(failure.messageKey, { values: failure.values })}</span>
				{#if failure.retryable}
					<button
						type="button"
						class="min-h-11 rounded-control border border-danger px-4 text-sm font-medium"
						onclick={startExport}
					>
						{$t('flow.package.retry')}
					</button>
				{/if}
			</div>
		{/if}

		{#if downloadFailure}
			<div
				class="mt-4 rounded-card border border-line bg-danger-soft p-4 text-sm text-danger"
				role="alert"
			>
				{$t(downloadFailure.messageKey, { values: downloadFailure.values })}
			</div>
		{/if}

		<h3 class="mt-6 text-sm font-semibold text-fg">
			{$t('flow.export.recentHeading')}
		</h3>
		{#if jobs.length === 0}
			<p class="mt-2 text-sm text-fg-muted">{$t('flow.export.recentEmpty')}</p>
		{:else}
			<ul class="mt-2 space-y-3" data-testid="flow-export-jobs">
				{#each jobs as view (view.jobId)}
					{@const job = view.job}
					<li
						class="rounded-card border border-line p-4 text-sm"
						data-testid="flow-export-job"
						data-job-id={view.jobId}
					>
						{#if job && job.status !== 'failed'}<PackageExportWarnings
								warnings={job.warnings}
								scopeExport
							/>{/if}
						<dl class="grid grid-cols-1 gap-2 md:grid-cols-2">
							<div>
								<dt class="text-fg-muted">{$t('flow.export.jobId')}</dt>
								<dd class="break-all font-mono text-xs text-fg">
									{view.jobId}
								</dd>
							</div>
							<div>
								<dt class="text-fg-muted">{$t('flow.export.status.label')}</dt>
								<dd class="text-fg" data-testid="flow-export-status">
									{job ? statusLabel(job.status) : $t('flow.export.status.loading')}
									{#if view.polling}
										<span class="text-fg-muted">
											{$t('flow.export.polling')}
										</span>
									{/if}
								</dd>
							</div>
							{#if job && job.status !== 'failed'}
								<div class="md:col-span-2">
									<dt class="text-fg-muted">{$t('flow.export.checksum')}</dt>
									<dd
										class="break-all font-mono text-xs text-fg"
										data-testid="flow-export-checksum"
									>
										{job.checksum}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">{$t('flow.export.size')}</dt>
									<dd class="text-fg">{formatBytes(job.size)}</dd>
								</div>
								<div>
									<dt class="text-fg-muted">{$t('flow.export.expiresAt')}</dt>
									<dd class="text-fg">{formatTime(job.expires_at)}</dd>
								</div>
							{/if}
						</dl>
						{#if view.failure}
							<p
								class="mt-2 text-sm text-danger"
								role="alert"
								data-testid="flow-export-job-error"
							>
								{$t(view.failure.messageKey, { values: view.failure.values })}
							</p>
						{/if}
						{#if exportDownloadUrl(job) && job}
							{@const finished = job}
							<button
								type="button"
								class="mt-3 min-h-11 rounded-control border border-fg px-4 text-sm font-medium text-fg hover:bg-surface disabled:text-control disabled:hover:bg-transparent"
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
