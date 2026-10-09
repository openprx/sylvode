<script lang="ts">
	// Package import wizard. Step gating, idempotency keys, error blocking, expiry and the exact
	// hash sent on commit live in `PackageImportWizard`; this component renders its snapshot and
	// holds only focus/announcement state. Package bytes go straight from the file input to the
	// upload and are never parsed or stored here.
	import { onDestroy, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import Card from '$lib/components/Card.svelte';
	import {
		CONFLICT_POLICY_KEYS,
		ESTIMATE_KIND_KEYS,
		EXTERNAL_POLICY_KEYS,
		FILE_REJECTION_KEYS,
		PACKAGE_FILE_EXTENSION,
		STEP_KEYS,
		UPLOAD_STATE_KEYS,
		IMPORT_ARCHIVE_BYTES_MAX,
		PackageImportWizard,
		WIZARD_STEPS,
		type ProjectMappingRow,
		type WizardSnapshot,
		type WizardStep
	} from '$lib/flow/package-wizard';
	import type { WorkspaceProjectOption } from '$lib/flow/package-export';

	interface Props {
		workspaceId: string;
		projects: WorkspaceProjectOption[];
	}

	let { workspaceId, projects }: Props = $props();

	// Snapshots are plain objects issued by the wizard; `$state.raw` keeps them unproxied.
	let snap = $state.raw<WizardSnapshot | null>(null);
	let secondsLeft = $state<number | null>(null);
	let liveKey = $state('');
	let liveValues = $state<Record<string, string>>({});
	let copied = $state(false);
	let stepHeading = $state<HTMLElement | null>(null);

	// The page is keyed by its route, so the workspace id is fixed for this component's lifetime.
	const wizard = new PackageImportWizard(
		untrack(() => workspaceId),
		(next) => {
			const previousStep = snap?.step;
			snap = next;
			secondsLeft = wizard.secondsUntilExpiry();
			if (previousStep !== undefined && previousStep !== next.step) void focusStep(next.step);
		}
	);
	snap = wizard.snapshot();

	const clock = setInterval(() => {
		if (!snap?.preview) return;
		snap = wizard.snapshot();
		secondsLeft = wizard.secondsUntilExpiry();
	}, 1000);
	onDestroy(() => {
		clearInterval(clock);
		wizard.cancelUpload();
	});

	const stepIndex = $derived(snap ? WIZARD_STEPS.indexOf(snap.step) : 0);

	function announce(key: string, values: Record<string, string> = {}) {
		liveKey = key;
		liveValues = values;
	}

	async function focusStep(step: WizardStep) {
		announce('flow.import.live.step', {
			n: String(WIZARD_STEPS.indexOf(step) + 1),
			total: String(WIZARD_STEPS.length),
			name: $t(STEP_KEYS[step])
		});
		await tick();
		stepHeading?.focus();
	}

	function formatBytes(size: number): string {
		if (size < 1024) return $t('flow.package.bytes', { values: { n: String(size) } });
		if (size < 1024 * 1024)
			return $t('flow.package.kib', { values: { n: (size / 1024).toFixed(1) } });
		return $t('flow.package.mib', { values: { n: (size / 1024 / 1024).toFixed(1) } });
	}

	function formatCountdown(seconds: number): string {
		const minutes = Math.floor(seconds / 60);
		const rest = seconds % 60;
		return `${minutes}:${String(rest).padStart(2, '0')}`;
	}

	function countEntries(value: unknown): number {
		return value && typeof value === 'object' ? Object.keys(value as object).length : 0;
	}

	function onFileChange(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0] ?? null;
		wizard.selectFile(file, file ? file.name : null);
	}

	async function upload() {
		announce('flow.import.live.uploading');
		const ok = await wizard.startUpload();
		if (ok) announce('flow.import.live.uploaded');
		else if (wizard.snapshot().uploadState === 'cancelled') announce('flow.import.live.cancelled');
	}

	function cancelUpload() {
		wizard.cancelUpload();
		announce('flow.import.live.cancelled');
	}

	function updateRow(index: number, patch: Partial<ProjectMappingRow>) {
		if (!snap) return;
		const rows = snap.options.projectMapping.map((row, i) =>
			i === index ? { ...row, ...patch } : row
		);
		wizard.setOptions({ projectMapping: rows });
	}

	function addRow() {
		if (!snap) return;
		wizard.setOptions({
			projectMapping: [
				...snap.options.projectMapping,
				{ sourceProjectId: '', targetProjectId: null }
			]
		});
	}

	function removeRow(index: number) {
		if (!snap) return;
		wizard.setOptions({
			projectMapping: snap.options.projectMapping.filter((_, i) => i !== index)
		});
	}

	async function next() {
		if (snap?.step === 'options') announce('flow.import.live.previewing');
		await wizard.advance();
	}

	async function rePreview() {
		announce('flow.import.live.previewing');
		await wizard.runPreview();
	}

	async function copyHash(hash: string) {
		try {
			await navigator.clipboard.writeText(hash);
			copied = true;
			announce('flow.import.confirm.copied');
		} catch {
			copied = false;
		}
	}

	async function commit() {
		const outcome = await wizard.commit();
		if (outcome.status === 'committed') {
			announce('flow.import.live.committed');
			await goto(
				resolve('/(app)/workspace/[workspaceId]/settings/flow/package/imports/[importId]', {
					workspaceId,
					importId: outcome.importId
				})
			);
		}
	}

	const buttonPrimary =
		'min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900';
	const buttonSecondary =
		'min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800';
	const fieldClass =
		'mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
</script>

<Card>
	<section aria-labelledby="flow-import-heading" data-testid="flow-import">
		<h2 id="flow-import-heading" class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.import.heading')}
		</h2>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">{$t('flow.import.description')}</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-import-live">
			{liveKey ? $t(liveKey, { values: liveValues }) : ''}
		</p>

		{#if snap}
			<nav class="mt-4" aria-label={$t('flow.import.stepsLabel')}>
				<ol class="flex flex-col gap-2 md:flex-row md:gap-1">
					{#each WIZARD_STEPS as step, index (step)}
						<li class="flex-1">
							<button
								type="button"
								class="flex min-h-11 w-full items-center gap-2 rounded-md border px-3 text-left text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:cursor-not-allowed {index ===
								stepIndex
									? 'border-blue-500 bg-blue-50 font-semibold text-blue-800 dark:bg-blue-950/40 dark:text-blue-200'
									: 'border-slate-200 text-slate-600 dark:border-slate-700 dark:text-slate-300'}"
								aria-current={index === stepIndex ? 'step' : undefined}
								disabled={index > stepIndex || snap.uploadState === 'uploading' || snap.committing}
								onclick={() => wizard.goTo(step)}
							>
								<span aria-hidden="true">{index + 1}.</span>
								<span>{$t(STEP_KEYS[step])}</span>
							</button>
						</li>
					{/each}
				</ol>
			</nav>

			<div class="mt-6">
				<h3
					bind:this={stepHeading}
					tabindex="-1"
					class="text-base font-semibold text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-100"
					data-testid="flow-import-step"
				>
					{$t('flow.import.stepOf', {
						values: {
							n: String(stepIndex + 1),
							total: String(WIZARD_STEPS.length),
							name: $t(STEP_KEYS[snap.step])
						}
					})}
				</h3>

				{#if snap.step === 'file'}
					<div class="mt-3 space-y-3">
						<label
							for="flow-import-file"
							class="block text-sm font-medium text-slate-700 dark:text-slate-300"
						>
							{$t('flow.import.file.label', { values: { ext: PACKAGE_FILE_EXTENSION } })}
						</label>
						<input
							id="flow-import-file"
							type="file"
							accept=".zip"
							class="block min-h-11 w-full text-sm text-slate-700 file:mr-3 file:min-h-11 file:rounded-md file:border-0 file:bg-slate-100 file:px-4 file:text-sm file:font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-200 dark:file:bg-slate-700"
							onchange={onFileChange}
						/>
						<p class="text-xs text-slate-500 dark:text-slate-400">
							{$t('flow.import.file.hint', {
								values: { max: formatBytes(IMPORT_ARCHIVE_BYTES_MAX) }
							})}
						</p>
						{#if snap.fileName !== null && snap.fileSize !== null}
							<p class="text-sm text-slate-900 dark:text-slate-100" data-testid="flow-import-file">
								{$t('flow.import.file.selected', {
									values: { name: snap.fileName, size: formatBytes(snap.fileSize) }
								})}
							</p>
						{/if}
						{#if snap.fileRejection}
							<p class="text-sm text-red-700 dark:text-red-300" role="alert">
								{$t(FILE_REJECTION_KEYS[snap.fileRejection], {
									values: {
										max: formatBytes(IMPORT_ARCHIVE_BYTES_MAX),
										ext: PACKAGE_FILE_EXTENSION
									}
								})}
							</p>
						{/if}
					</div>
				{:else if snap.step === 'upload'}
					<div class="mt-3 space-y-3">
						<p class="text-sm text-slate-900 dark:text-slate-100">
							{$t('flow.import.file.selected', {
								values: { name: snap.fileName ?? '', size: formatBytes(snap.fileSize ?? 0) }
							})}
						</p>
						{#if snap.progress}
							<div>
								<label
									for="flow-import-progress"
									class="block text-sm text-slate-700 dark:text-slate-300"
								>
									{$t('flow.import.upload.progress', {
										values: {
											loaded: formatBytes(snap.progress.loaded),
											total: formatBytes(snap.progress.total ?? snap.fileSize ?? 0)
										}
									})}
								</label>
								<progress
									id="flow-import-progress"
									class="mt-1 h-2 w-full"
									max={snap.progress.total ?? snap.fileSize ?? 1}
									value={snap.progress.loaded}
								></progress>
							</div>
						{/if}
						<p
							class="text-sm text-slate-700 dark:text-slate-300"
							data-testid="flow-import-upload-state"
						>
							{$t(UPLOAD_STATE_KEYS[snap.uploadState])}
						</p>
						{#if snap.artifact}
							<dl class="grid grid-cols-1 gap-2 text-sm">
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.packageSha256')}
									</dt>
									<dd class="break-all font-mono text-xs text-slate-900 dark:text-slate-100">
										{snap.artifact.package_sha256}
									</dd>
								</div>
							</dl>
						{/if}
						<div class="flex flex-col gap-3 sm:flex-row">
							{#if snap.uploadState === 'uploading'}
								<button type="button" class={buttonSecondary} onclick={cancelUpload}>
									{$t('flow.import.upload.cancel')}
								</button>
							{:else if snap.uploadState !== 'uploaded'}
								<button
									type="button"
									class={buttonPrimary}
									disabled={snap.blockingFailure !== null}
									onclick={upload}
								>
									{snap.uploadState === 'idle'
										? $t('flow.import.upload.start')
										: $t('flow.import.upload.restart')}
								</button>
							{/if}
						</div>
					</div>
				{:else if snap.step === 'options'}
					<div class="mt-3 space-y-5">
						<fieldset>
							<legend class="text-sm font-medium text-slate-700 dark:text-slate-300">
								{$t('flow.import.options.externalLegend')}
							</legend>
							{#each ['reject', 'detach'] as const as policy (policy)}
								<label class="mt-1 flex min-h-11 cursor-pointer items-center gap-3 text-sm">
									<input
										type="radio"
										name="flow-import-external"
										class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
										checked={snap.options.externalReferencePolicy === policy}
										onchange={() => wizard.setOptions({ externalReferencePolicy: policy })}
									/>
									<span class="text-slate-900 dark:text-slate-100">
										{$t(EXTERNAL_POLICY_KEYS[policy])}
									</span>
								</label>
							{/each}
						</fieldset>
						<fieldset>
							<legend class="text-sm font-medium text-slate-700 dark:text-slate-300">
								{$t('flow.import.options.conflictLegend')}
							</legend>
							{#each ['reject_existing', 'reuse_import_lineage'] as const as policy (policy)}
								<label class="mt-1 flex min-h-11 cursor-pointer items-center gap-3 text-sm">
									<input
										type="radio"
										name="flow-import-conflict"
										class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
										checked={snap.options.conflictPolicy === policy}
										onchange={() => wizard.setOptions({ conflictPolicy: policy })}
									/>
									<span class="text-slate-900 dark:text-slate-100">
										{$t(CONFLICT_POLICY_KEYS[policy])}
									</span>
								</label>
							{/each}
						</fieldset>
						<label class="flex min-h-11 cursor-pointer items-center gap-3 text-sm">
							<input
								type="checkbox"
								class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
								checked={snap.options.includeHistory}
								onchange={(event) =>
									wizard.setOptions({ includeHistory: event.currentTarget.checked })}
							/>
							<span class="text-slate-900 dark:text-slate-100">
								{$t('flow.import.options.includeHistory')}
							</span>
						</label>
						<fieldset>
							<legend class="text-sm font-medium text-slate-700 dark:text-slate-300">
								{$t('flow.import.options.mappingLegend')}
							</legend>
							<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
								{$t('flow.import.options.mappingHint')}
							</p>
							{#each snap.options.projectMapping as row, index (index)}
								<div class="mt-2 grid grid-cols-1 gap-2 md:grid-cols-[1fr_1fr_auto] md:items-end">
									<div>
										<label
											for={`flow-import-source-${index}`}
											class="block text-xs text-slate-600 dark:text-slate-300"
										>
											{$t('flow.import.options.sourceProject')}
										</label>
										<input
											id={`flow-import-source-${index}`}
											class={fieldClass}
											value={row.sourceProjectId}
											oninput={(event) =>
												updateRow(index, { sourceProjectId: event.currentTarget.value })}
										/>
									</div>
									<div>
										<label
											for={`flow-import-target-${index}`}
											class="block text-xs text-slate-600 dark:text-slate-300"
										>
											{$t('flow.import.options.targetProject')}
										</label>
										<select
											id={`flow-import-target-${index}`}
											class={fieldClass}
											value={row.targetProjectId ?? ''}
											onchange={(event) =>
												updateRow(index, {
													targetProjectId: event.currentTarget.value || null
												})}
										>
											<option value="">{$t('flow.import.options.unprojected')}</option>
											{#each projects as project (project.id)}
												<option value={project.id}>{project.label}</option>
											{/each}
										</select>
									</div>
									<button type="button" class={buttonSecondary} onclick={() => removeRow(index)}>
										{$t('flow.import.options.removeMapping')}
									</button>
								</div>
							{/each}
							<button type="button" class="{buttonSecondary} mt-2" onclick={addRow}>
								{$t('flow.import.options.addMapping')}
							</button>
							{#if !snap.optionsValid}
								<p class="mt-2 text-sm text-red-700 dark:text-red-300" role="alert">
									{$t('flow.import.options.mappingInvalid')}
								</p>
							{/if}
						</fieldset>
					</div>
				{:else if snap.step === 'preview'}
					<div class="mt-3 space-y-4" data-testid="flow-import-preview">
						{#if snap.previewState === 'running'}
							<p class="text-sm text-slate-600 dark:text-slate-300">
								{$t('flow.import.preview.running')}
							</p>
						{/if}
						{#if snap.preview}
							{@const preview = snap.preview}
							<dl class="grid grid-cols-1 gap-3 text-sm md:grid-cols-2">
								<div class="md:col-span-2">
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.packageSha256')}
									</dt>
									<dd
										class="break-all font-mono text-xs text-slate-900 dark:text-slate-100"
										data-testid="flow-import-preview-sha"
									>
										{preview.package_sha256}
									</dd>
								</div>
								<div class="md:col-span-2">
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.mappingHash')}
									</dt>
									<dd class="break-all font-mono text-xs text-slate-900 dark:text-slate-100">
										{preview.mapping_hash}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.packageId')}
									</dt>
									<dd class="break-all font-mono text-xs text-slate-900 dark:text-slate-100">
										{preview.package_id}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.objectMapping')}
									</dt>
									<dd
										class="text-slate-900 dark:text-slate-100"
										data-testid="flow-import-preview-objects"
									>
										{countEntries(preview.mapping.object_map)}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.estimated')}
									</dt>
									<dd class="text-slate-900 dark:text-slate-100">
										<ul>
											{#each Object.entries(preview.estimated_changes) as [kind, count] (kind)}
												<li>
													{ESTIMATE_KIND_KEYS[kind]
														? $t(ESTIMATE_KIND_KEYS[kind], { values: { n: String(count) } })
														: `${kind}: ${count}`}
												</li>
											{/each}
										</ul>
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.expiresIn')}
									</dt>
									<dd class="text-slate-900 dark:text-slate-100" data-testid="flow-import-expiry">
										{snap.expired
											? $t('flow.import.preview.expired')
											: formatCountdown(secondsLeft ?? 0)}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.conflicts')}
									</dt>
									<dd
										class="text-slate-900 dark:text-slate-100"
										data-testid="flow-import-conflicts"
									>
										{#if preview.conflicts.length === 0}
											{$t('flow.import.preview.none')}
										{:else}
											<ul class="break-all font-mono text-xs">
												{#each preview.conflicts as conflict (conflict)}<li>{conflict}</li>{/each}
											</ul>
										{/if}
									</dd>
								</div>
								<div>
									<dt class="text-slate-500 dark:text-slate-400">
										{$t('flow.import.preview.warnings')}
									</dt>
									<dd class="text-slate-900 dark:text-slate-100">
										{#if preview.warnings.length === 0}
											{$t('flow.import.preview.none')}
										{:else}
											<ul>
												{#each preview.warnings as warning (warning)}<li>{warning}</li>{/each}
											</ul>
										{/if}
									</dd>
								</div>
							</dl>
						{/if}
						{#if snap.preview || snap.previewState === 'failed'}
							<button
								type="button"
								class={buttonSecondary}
								disabled={snap.previewState === 'running'}
								onclick={rePreview}
							>
								{$t('flow.import.preview.rerun')}
							</button>
						{/if}
					</div>
				{:else if snap.step === 'confirm' && snap.preview}
					{@const preview = snap.preview}
					<div class="mt-3 space-y-4">
						<div>
							<label
								for="flow-import-confirm-hash"
								class="block text-sm font-medium text-slate-700 dark:text-slate-300"
							>
								{$t('flow.import.packageSha256')}
							</label>
							<div class="mt-1 flex flex-col gap-2 sm:flex-row">
								<input
									id="flow-import-confirm-hash"
									class="{fieldClass} font-mono text-xs"
									readonly
									value={preview.package_sha256}
									data-testid="flow-import-confirm-hash"
								/>
								<button
									type="button"
									class={buttonSecondary}
									onclick={() => void copyHash(preview.package_sha256)}
								>
									{copied ? $t('flow.import.confirm.copied') : $t('flow.import.confirm.copy')}
								</button>
							</div>
						</div>
						<p class="text-sm text-slate-700 dark:text-slate-300">
							{$t('flow.import.confirm.summary', {
								values: {
									objects: String(countEntries(preview.mapping.object_map)),
									policy: $t(CONFLICT_POLICY_KEYS[snap.options.conflictPolicy])
								}
							})}
						</p>
						<label class="flex min-h-11 cursor-pointer items-center gap-3 text-sm">
							<input
								type="checkbox"
								class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
								checked={snap.hashAcknowledged}
								disabled={snap.committing || snap.expired}
								onchange={(event) => wizard.acknowledgeHash(event.currentTarget.checked)}
							/>
							<span class="text-slate-900 dark:text-slate-100"
								>{$t('flow.import.confirm.acknowledge')}</span
							>
						</label>
						{#if snap.expired}
							<p class="text-sm text-amber-700 dark:text-amber-300" role="alert">
								{$t('flow.import.confirm.expired')}
							</p>
						{:else}
							<p class="text-xs text-slate-500 dark:text-slate-400">
								{$t('flow.import.confirm.expiresIn', {
									values: { time: formatCountdown(secondsLeft ?? 0) }
								})}
							</p>
						{/if}
						<button type="button" class={buttonPrimary} disabled={!snap.canCommit} onclick={commit}>
							{snap.committing
								? $t('flow.import.confirm.committing')
								: $t('flow.import.confirm.commit')}
						</button>
					</div>
				{/if}

				{#if snap.blockingFailure}
					<div
						class="mt-4 rounded-lg border border-red-300 bg-red-50 p-4 text-sm text-red-800 dark:border-red-800 dark:bg-red-950/30 dark:text-red-200"
						role="alert"
						data-testid="flow-import-blocking"
					>
						<p class="font-medium">{$t('flow.import.blockingTitle')}</p>
						<p class="mt-1">
							{$t(snap.blockingFailure.messageKey, { values: snap.blockingFailure.values })}
						</p>
					</div>
				{:else if snap.failure}
					<div
						class="mt-4 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700 dark:border-red-900 dark:bg-red-950/30 dark:text-red-300"
						role="alert"
						data-testid="flow-import-error"
					>
						{$t(snap.failure.messageKey, { values: snap.failure.values })}
					</div>
				{/if}

				<div class="mt-6 flex flex-col-reverse gap-3 sm:flex-row sm:justify-between">
					<button
						type="button"
						class={buttonSecondary}
						disabled={stepIndex === 0 || snap.uploadState === 'uploading' || snap.committing}
						onclick={() => wizard.back()}
					>
						{$t('flow.import.back')}
					</button>
					{#if snap.step !== 'confirm'}
						<button
							type="button"
							class={buttonPrimary}
							disabled={!snap.canAdvance || snap.previewState === 'running'}
							onclick={next}
						>
							{snap.step === 'options' ? $t('flow.import.runPreview') : $t('flow.import.next')}
						</button>
					{/if}
				</div>
			</div>
		{/if}
	</section>
</Card>
