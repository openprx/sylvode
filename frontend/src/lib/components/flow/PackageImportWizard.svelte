<script lang="ts">
	// Package import wizard. Step gating, idempotency keys, error blocking, expiry and the exact
	// hash sent on commit live in `PackageImportWizard`; this component renders its snapshot and
	// holds only focus/announcement state. Package bytes go straight from the file input to the
	// upload and are never parsed or stored here.
	import { onDestroy, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import Card from '#lib/components/Card.svelte';
	import {
		CONFLICT_POLICY_KEYS,
		ESTIMATE_KIND_KEYS,
		EXTERNAL_POLICY_KEYS,
		FILE_REJECTION_KEYS,
		PACKAGE_FILE_EXTENSION,
		STEP_KEYS,
		UPLOAD_STATE_KEYS,
		IMPORT_ARCHIVE_BYTES_MAX,
		PREVIEW_HOLD_KEYS,
		PackageImportWizard,
		WIZARD_STEPS,
		previewConflicts,
		type ProjectMappingRow,
		type WizardSnapshot,
		type WizardStep
	} from '#lib/flow/package-wizard.js';
	import type { WorkspaceProjectOption } from '#lib/flow/package-export.js';

	interface Props {
		scopeId: string;
		reportWorkspaceId?: string;
		projects: WorkspaceProjectOption[];
		allowHistory?: boolean;
	}

	let { scopeId, projects, reportWorkspaceId, allowHistory = true }: Props = $props();

	// Snapshots are plain objects issued by the wizard; `$state.raw` keeps them unproxied.
	let snap = $state.raw<WizardSnapshot | null>(null);
	let secondsLeft = $state<number | null>(null);
	let liveKey = $state('');
	let liveValues = $state<Record<string, string>>({});
	let copied = $state(false);
	let stepHeading = $state<HTMLElement | null>(null);

	// The page is keyed by its route, so the workspace id is fixed for this component's lifetime.
	const wizard = new PackageImportWizard(
		untrack(() => scopeId),
		(next) => {
			const previousStep = snap?.step;
			snap = next;
			secondsLeft = wizard.secondsUntilExpiry();
			if (previousStep !== undefined && previousStep !== next.step) void focusStep(next.step);
		}
	);
	snap = wizard.snapshot();
	$effect(() => {
		if (!allowHistory && snap?.options.includeHistory) wizard.setOptions({ includeHistory: false });
	});

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

	/** The mapping select's value: a project id, `unprojected`, or `''` while the user has not
	 * chosen a target for a row the preview added. */
	function targetValue(row: ProjectMappingRow): string {
		if (row.unresolved) return '';
		return row.targetProjectId ?? 'unprojected';
	}

	function chooseTarget(index: number, value: string) {
		if (value === '') return;
		updateRow(index, {
			targetProjectId: value === 'unprojected' ? null : value,
			unresolved: false
		});
	}

	function projectLabel(sourceProjectId: string, title: string | null | undefined): string {
		return title && title.trim() !== '' ? title : sourceProjectId;
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

	function announceUnmapped() {
		const current = wizard.snapshot();
		if (current.step === 'options' && current.unmappedProjects.length > 0) {
			announce('flow.import.live.unmapped', { n: String(current.unmappedProjects.length) });
		}
	}

	async function next() {
		if (snap?.step === 'options') announce('flow.import.live.previewing');
		await wizard.advance();
		announceUnmapped();
	}

	async function rePreview() {
		announce('flow.import.live.previewing');
		await wizard.runPreview();
		announceUnmapped();
	}

	async function previewWithReuse() {
		announce('flow.import.live.previewing');
		await wizard.previewWithReuse();
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
				reportWorkspaceId
					? resolve('/(app)/workspace/[workspaceId]/settings/flow/package/imports/[importId]', {
							workspaceId: reportWorkspaceId,
							importId: outcome.importId
						})
					: resolve('/(app)/flow/package/imports/[importId]', { importId: outcome.importId })
			);
		}
	}

	const buttonPrimary =
		'min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50 motion-safe:transition-colors';
	const buttonSecondary =
		'min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-fg hover:bg-slate-100 disabled:opacity-50 dark:border-slate-600 dark:hover:bg-slate-800';
	const fieldClass =
		'mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
</script>

<p
	class="mb-4 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-950 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100"
	data-testid="flow-import-snapshot-history-notice"
>
	{$t('flow.package.snapshotHistoryNotice')}
</p>

<Card>
	<section aria-labelledby="flow-import-heading" data-testid="flow-import">
		<h2 id="flow-import-heading" class="text-lg font-semibold text-fg">
			{$t('flow.import.heading')}
		</h2>
		<p class="mt-1 text-sm text-fg-muted">{$t('flow.import.description')}</p>

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
								class="flex min-h-11 w-full items-center gap-2 rounded-md border px-3 text-left text-sm disabled:cursor-not-allowed {index ===
								stepIndex
									? 'border-blue-500 bg-blue-50 font-semibold text-blue-800 dark:bg-blue-950/40 dark:text-blue-200'
									: 'border-line text-fg-muted'}"
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
					class="text-base font-semibold text-fg"
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
							class="block text-sm font-medium text-fg"
						>
							{$t('flow.import.file.label', { values: { ext: PACKAGE_FILE_EXTENSION } })}
						</label>
						<input
							id="flow-import-file"
							type="file"
							accept=".zip"
							class="block min-h-11 w-full text-sm text-fg file:mr-3 file:min-h-11 file:rounded-md file:border-0 file:bg-slate-100 file:px-4 file:text-sm file:font-medium dark:file:bg-slate-700"
							onchange={onFileChange}
						/>
						<p class="text-xs text-fg-muted">
							{$t('flow.import.file.hint', {
								values: { max: formatBytes(IMPORT_ARCHIVE_BYTES_MAX) }
							})}
						</p>
						{#if snap.fileName !== null && snap.fileSize !== null}
							<p class="text-sm text-fg" data-testid="flow-import-file">
								{$t('flow.import.file.selected', {
									values: { name: snap.fileName, size: formatBytes(snap.fileSize) }
								})}
							</p>
						{/if}
						{#if snap.fileRejection}
							<p class="text-sm text-danger" role="alert">
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
						<p class="text-sm text-fg">
							{$t('flow.import.file.selected', {
								values: { name: snap.fileName ?? '', size: formatBytes(snap.fileSize ?? 0) }
							})}
						</p>
						{#if snap.progress}
							<div>
								<label
									for="flow-import-progress"
									class="block text-sm text-fg"
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
							class="text-sm text-fg"
							data-testid="flow-import-upload-state"
						>
							{$t(UPLOAD_STATE_KEYS[snap.uploadState])}
						</p>
						{#if snap.artifact}
							<dl class="grid grid-cols-1 gap-2 text-sm">
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.packageSha256')}
									</dt>
									<dd class="break-all font-mono text-xs text-fg">
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
							<legend class="text-sm font-medium text-fg">
								{$t('flow.import.options.externalLegend')}
							</legend>
							{#each ['reject', 'detach'] as const as policy (policy)}
								<label class="mt-1 flex min-h-11 cursor-pointer items-center gap-3 text-sm">
									<input
										type="radio"
										name="flow-import-external"
										class="h-5 w-5"
										checked={snap.options.externalReferencePolicy === policy}
										onchange={() => wizard.setOptions({ externalReferencePolicy: policy })}
									/>
									<span class="text-fg">
										{$t(EXTERNAL_POLICY_KEYS[policy])}
									</span>
								</label>
							{/each}
						</fieldset>
						<fieldset>
							<legend class="text-sm font-medium text-fg">
								{$t('flow.import.options.conflictLegend')}
							</legend>
							{#each ['reject_existing', 'reuse_import_lineage'] as const as policy (policy)}
								<label class="mt-1 flex min-h-11 cursor-pointer items-center gap-3 text-sm">
									<input
										type="radio"
										name="flow-import-conflict"
										class="h-5 w-5"
										checked={snap.options.conflictPolicy === policy}
										onchange={() => wizard.setOptions({ conflictPolicy: policy })}
									/>
									<span class="text-fg">
										{$t(CONFLICT_POLICY_KEYS[policy])}
									</span>
								</label>
							{/each}
						</fieldset>
						<label class="flex min-h-11 cursor-pointer items-center gap-3 text-sm">
							<input
								type="checkbox"
								class="h-5 w-5"
								checked={snap.options.includeHistory}
								disabled={!allowHistory}
								onchange={(event) =>
									wizard.setOptions({
										includeHistory: allowHistory && event.currentTarget.checked
									})}
							/>
							<span class="text-fg">
								{$t('flow.import.options.includeHistory')}
							</span>
						</label>
						{#if !allowHistory}<p class="text-xs text-slate-500">
								{$t('flow.personal.packageHistoryUnavailable')}
							</p>{/if}
						<fieldset>
							<legend class="text-sm font-medium text-fg">
								{$t('flow.import.options.mappingLegend')}
							</legend>
							<p class="mt-1 text-xs text-fg-muted">
								{$t('flow.import.options.mappingHint')}
							</p>
							{#if snap.unmappedProjects.length > 0}
								<div
									class="mt-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
									role="alert"
									data-testid="flow-import-unmapped"
								>
									<p>{$t('flow.import.options.unmappedNotice')}</p>
									<ul class="mt-1 list-disc pl-5">
										{#each snap.unmappedProjects as project (project.source_project_id)}
											<li data-testid="flow-import-unmapped-project">
												{projectLabel(project.source_project_id, project.title)}
											</li>
										{/each}
									</ul>
								</div>
							{/if}
							{#each snap.options.projectMapping as row, index (index)}
								<div
									class="mt-2 grid grid-cols-1 gap-2 md:grid-cols-[1fr_1fr_auto] md:items-end"
									data-testid="flow-import-mapping-row"
								>
									<div>
										{#if row.title !== undefined}
											<p class="block text-xs text-fg-muted">
												{$t('flow.import.options.sourceProject')}
											</p>
											<p
												class="mt-1 flex min-h-11 flex-col justify-center text-sm text-fg"
												data-testid="flow-import-mapping-source"
											>
												<span>{projectLabel(row.sourceProjectId, row.title)}</span>
												<span class="break-all font-mono text-xs text-fg-muted"
													>{row.sourceProjectId}</span
												>
											</p>
										{:else}
											<label
												for={`flow-import-source-${index}`}
												class="block text-xs text-fg-muted"
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
										{/if}
									</div>
									<div>
										<label
											for={`flow-import-target-${index}`}
											class="block text-xs text-fg-muted"
										>
											{$t('flow.import.options.targetProject')}
										</label>
										<select
											id={`flow-import-target-${index}`}
											class={fieldClass}
											aria-label={$t('flow.import.options.targetFor', {
												values: { project: projectLabel(row.sourceProjectId, row.title) }
											})}
											value={targetValue(row)}
											onchange={(event) => chooseTarget(index, event.currentTarget.value)}
										>
											{#if row.unresolved}
												<option value="" disabled>{$t('flow.import.options.chooseTarget')}</option>
											{/if}
											<option value="unprojected">{$t('flow.import.options.unprojected')}</option>
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
								<p class="mt-2 text-sm text-danger" role="alert">
									{$t('flow.import.options.mappingInvalid')}
								</p>
							{/if}
						</fieldset>
					</div>
				{:else if snap.step === 'preview'}
					<div class="mt-3 space-y-4" data-testid="flow-import-preview">
						{#if snap.previewState === 'running'}
							<p class="text-sm text-fg-muted">
								{$t('flow.import.preview.running')}
							</p>
						{/if}
						{#if snap.preview}
							{@const preview = snap.preview}
							{@const conflicts = previewConflicts(preview)}
							{@const summary = preview.manifest_summary}
							{#if snap.previewHold}
								<div
									class="space-y-3 rounded-md border border-amber-300 bg-amber-50 p-4 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-100"
									role="alert"
									data-testid="flow-import-hold"
									data-hold={snap.previewHold}
								>
									<p>{$t(PREVIEW_HOLD_KEYS[snap.previewHold])}</p>
									{#if snap.previewHold === 'lineage_exists'}
										<button
											type="button"
											class={buttonSecondary}
											disabled={snap.previewState === 'running'}
											onclick={previewWithReuse}
											data-testid="flow-import-reuse"
										>
											{$t('flow.import.preview.reuseAction')}
										</button>
									{/if}
								</div>
							{/if}
							{#if summary}
								<section
									aria-labelledby="flow-import-manifest-heading"
									class="rounded-md border border-line p-3 text-sm"
									data-testid="flow-import-manifest"
								>
									<h4
										id="flow-import-manifest-heading"
										class="font-medium text-fg"
									>
										{$t('flow.import.preview.manifest.heading')}
									</h4>
									<dl class="mt-2 grid grid-cols-1 gap-2 md:grid-cols-2">
										<div>
											<dt class="text-fg-muted">
												{$t('flow.import.preview.manifest.objects')}
											</dt>
											<dd
												class="text-fg"
												data-testid="flow-import-manifest-objects"
											>
												{summary.object_count}
											</dd>
										</div>
										<div>
											<dt class="text-fg-muted">
												{$t('flow.import.preview.manifest.documents')}
											</dt>
											<dd
												class="text-fg"
												data-testid="flow-import-manifest-documents"
											>
												{summary.document_count}
											</dd>
										</div>
										<div>
											<dt class="text-fg-muted">
												{$t('flow.import.preview.manifest.schema')}
											</dt>
											<dd
												class="text-fg"
												data-testid="flow-import-manifest-schema"
											>
												{summary.package_schema}
											</dd>
										</div>
										<div>
											<dt class="text-fg-muted">
												{$t('flow.import.preview.manifest.engine')}
											</dt>
											<dd
												class="break-all text-fg"
												data-testid="flow-import-manifest-engine"
											>
												{$t('flow.import.preview.manifest.engineValue', {
													values: {
														name: summary.engine.name,
														version: summary.engine.crate_version,
														wire: String(summary.engine.wire_format_version)
													}
												})}
											</dd>
										</div>
										{#if summary.source}
											<div class="md:col-span-2">
												<dt class="text-fg-muted">
													{$t('flow.package.sourceProvenance')}
												</dt>
												<dd
													class="break-all font-mono text-xs text-fg"
													data-testid="flow-import-source-provenance"
												>
													{#if summary.source.scope_id}<span class="block"
															>{$t('flow.package.sourceScope')}: {summary.source.scope_id}</span
														>{/if}
													{#if summary.source.workspace_id}<span class="block"
															>{$t('flow.package.sourceWorkspace')}: {summary.source
																.workspace_id}</span
														>{/if}
												</dd>
											</div>
										{/if}
										<div class="md:col-span-2">
											<dt class="text-fg-muted">
												{$t('flow.import.preview.manifest.projects')}
											</dt>
											<dd class="text-fg">
												{#if summary.projects.length === 0}
													{$t('flow.import.preview.manifest.noProjects')}
												{:else}
													<ul>
														{#each summary.projects as project (project.source_project_id)}
															<li data-testid="flow-import-manifest-project">
																{projectLabel(project.source_project_id, project.title)}
															</li>
														{/each}
													</ul>
												{/if}
											</dd>
										</div>
									</dl>
								</section>
							{/if}
							<dl class="grid grid-cols-1 gap-3 text-sm md:grid-cols-2">
								<div class="md:col-span-2">
									<dt class="text-fg-muted">
										{$t('flow.import.packageSha256')}
									</dt>
									<dd
										class="break-all font-mono text-xs text-fg"
										data-testid="flow-import-preview-sha"
									>
										{preview.package_sha256}
									</dd>
								</div>
								<div class="md:col-span-2">
									<dt class="text-fg-muted">
										{$t('flow.import.mappingHash')}
									</dt>
									<dd class="break-all font-mono text-xs text-fg">
										{preview.mapping_hash}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.preview.packageId')}
									</dt>
									<dd class="break-all font-mono text-xs text-fg">
										{preview.package_id}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.preview.objectMapping')}
									</dt>
									<dd
										class="text-fg"
										data-testid="flow-import-preview-objects"
									>
										{countEntries(preview.mapping.object_map)}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.preview.estimated')}
									</dt>
									<dd class="text-fg">
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
									<dt class="text-fg-muted">
										{$t('flow.import.preview.expiresIn')}
									</dt>
									<dd class="text-fg" data-testid="flow-import-expiry">
										{snap.expired
											? $t('flow.import.preview.expired')
											: formatCountdown(secondsLeft ?? 0)}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.preview.conflicts')}
									</dt>
									<dd
										class="text-fg"
										data-testid="flow-import-conflicts"
									>
										{#if conflicts.lineage.length === 0 && conflicts.unmapped.length === 0}
											{#if preview.conflicts.length === 0}
												{$t('flow.import.preview.none')}
											{:else}
												<ul class="break-all font-mono text-xs">
													{#each preview.conflicts as conflict (conflict)}<li>{conflict}</li>{/each}
												</ul>
											{/if}
										{:else}
											<ul class="space-y-1">
												{#each conflicts.lineage as conflict (conflict.source_object_id)}
													<li class="break-all text-xs" data-testid="flow-import-conflict-lineage">
														{$t('flow.import.preview.conflict.lineage', {
															values: {
																source: conflict.source_object_id,
																target: conflict.existing_target_id
															}
														})}
													</li>
												{/each}
												{#each conflicts.unmapped as conflict (conflict.source_project_id)}
													<li class="break-all text-xs" data-testid="flow-import-conflict-unmapped">
														{$t('flow.import.preview.conflict.unmapped', {
															values: {
																project: projectLabel(conflict.source_project_id, conflict.title)
															}
														})}
													</li>
												{/each}
											</ul>
										{/if}
									</dd>
								</div>
								<div>
									<dt class="text-fg-muted">
										{$t('flow.import.preview.warnings')}
									</dt>
									<dd class="text-fg">
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
								class="block text-sm font-medium text-fg"
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
						<p class="text-sm text-fg">
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
								class="h-5 w-5"
								checked={snap.hashAcknowledged}
								disabled={snap.committing || snap.expired}
								onchange={(event) => wizard.acknowledgeHash(event.currentTarget.checked)}
							/>
							<span class="text-fg"
								>{$t('flow.import.confirm.acknowledge')}</span
							>
						</label>
						{#if snap.expired}
							<p class="text-sm text-warning" role="alert">
								{$t('flow.import.confirm.expired')}
							</p>
						{:else}
							<p class="text-xs text-fg-muted">
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
						class="mt-4 rounded-lg border border-red-300 bg-red-50 p-4 text-sm text-danger dark:border-red-800 dark:bg-red-950/30"
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
						class="mt-4 rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-danger dark:border-red-900 dark:bg-red-950/30"
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
