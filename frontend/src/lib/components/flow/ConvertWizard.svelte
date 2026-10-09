<script lang="ts">
	// Flow Page -> Forms record conversion wizard. Step gating, the frozen preview, expiry, the
	// stale-frontier recovery, idempotency keys and the commit's preview-derived fields live in
	// `ConvertWizard`; this component renders its snapshot and holds only focus/announcement state.
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import Card from '$lib/components/Card.svelte';
	import type { FlowCommandService } from '$lib/flow/command-service';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import {
		CONVERT_STEPS,
		ConvertWizard,
		FIELD_PROBLEM_KEYS,
		STEP_KEYS,
		permissionExplanation,
		type ConvertSnapshot,
		type ConvertStep,
		type MappingField
	} from '$lib/flow/convert-wizard';

	interface Props {
		workspaceId: string;
		objectId: string;
		repository: Pick<FlowObjectRepository, 'get'>;
		commands: Pick<FlowCommandService, 'convertPreview' | 'convertCommit'>;
	}

	let { workspaceId, objectId, repository, commands }: Props = $props();

	// Snapshots are plain objects issued by the wizard; `$state.raw` keeps them unproxied.
	let snap = $state.raw<ConvertSnapshot | null>(null);
	let secondsLeft = $state<number | null>(null);
	let liveKey = $state('');
	let liveValues = $state<Record<string, string>>({});
	let stepHeading = $state<HTMLElement | null>(null);

	// The page is keyed by its route, so workspace and object are fixed for this component.
	const wizard = new ConvertWizard(
		untrack(() => workspaceId),
		untrack(() => objectId),
		untrack(() => ({ repository, commands })),
		(next) => {
			const previousStep = snap?.step;
			snap = next;
			secondsLeft = wizard.secondsUntilExpiry();
			if (previousStep !== undefined && previousStep !== next.step) void focusStep(next.step);
		}
	);
	snap = wizard.snapshot();

	onMount(() => {
		void wizard.load();
	});

	const clock = setInterval(() => {
		if (!snap?.preview) return;
		snap = wizard.snapshot();
		secondsLeft = wizard.secondsUntilExpiry();
	}, 1000);
	onDestroy(() => clearInterval(clock));

	const stepIndex = $derived(snap ? CONVERT_STEPS.indexOf(snap.step) : 0);
	const selectedForm = $derived(snap?.forms.find((form) => form.id === snap?.formId) ?? null);
	const selectedProject = $derived(
		snap?.projects.find((project) => project.id === snap?.projectId) ?? null
	);

	function announce(key: string, values: Record<string, string> = {}) {
		liveKey = key;
		liveValues = values;
	}

	async function focusStep(step: ConvertStep) {
		announce('flow.bridge.convert.live.step', {
			n: String(CONVERT_STEPS.indexOf(step) + 1),
			total: String(CONVERT_STEPS.length),
			name: $t(STEP_KEYS[step])
		});
		await tick();
		stepHeading?.focus();
	}

	function formatCountdown(seconds: number): string {
		const minutes = Math.floor(seconds / 60);
		const rest = seconds % 60;
		return `${minutes}:${String(rest).padStart(2, '0')}`;
	}

	function formatTime(value: string): string {
		const parsed = Date.parse(value);
		return Number.isFinite(parsed) ? new Date(parsed).toLocaleString() : value;
	}

	function inputValue(field: MappingField): string {
		const value = snap?.inputs[field.key];
		return typeof value === 'string' ? value : '';
	}

	function selectedOptions(field: MappingField): readonly string[] {
		const value = snap?.inputs[field.key];
		return Array.isArray(value) ? value : [];
	}

	function toggleOption(field: MappingField, option: string, checked: boolean) {
		const current = selectedOptions(field);
		const next = checked
			? [...current.filter((item) => item !== option), option]
			: current.filter((item) => item !== option);
		wizard.setValue(field.key, next);
	}

	function previewValue(value: unknown): string {
		if (Array.isArray(value)) return value.map((item) => String(item)).join(', ');
		if (typeof value === 'boolean')
			return value ? $t('flow.bridge.convert.value.yes') : $t('flow.bridge.convert.value.no');
		return String(value);
	}

	function previewValues(mapping: Record<string, unknown>): Array<[string, unknown]> {
		const values = mapping.values;
		return values && typeof values === 'object' ? Object.entries(values) : [];
	}

	function fieldLabel(key: string): string {
		return snap?.fields.find((field) => field.key === key)?.label ?? key;
	}

	async function next() {
		if (snap?.step === 'mapping') announce('flow.bridge.convert.live.previewing');
		await wizard.advance();
		announcePreviewOutcome();
	}

	async function rePreview() {
		announce('flow.bridge.convert.live.previewing');
		await wizard.runPreview();
		announcePreviewOutcome();
	}

	async function refreshAndPreview() {
		announce('flow.bridge.convert.live.refreshing');
		await wizard.refreshSourceAndPreview();
		announcePreviewOutcome();
	}

	function announcePreviewOutcome() {
		const current = wizard.snapshot();
		if (current.step !== 'preview') return;
		if (current.previewState === 'ready') announce('flow.bridge.convert.live.previewReady');
		else if (current.previewFailure) announce(current.previewFailure.messageKey);
	}

	async function commit() {
		announce('flow.bridge.convert.live.committing');
		const outcome = await wizard.commit();
		if (outcome.status === 'committed') {
			announce('flow.bridge.convert.live.committed');
			// The conversion job page is FP-N5's route; it is not in the route tree yet, so the typed
			// `resolve()` cannot name it.
			// eslint-disable-next-line svelte/no-navigation-without-resolve
			await goto(`/workspace/${workspaceId}/flow/conversions/${outcome.jobId}`);
		} else if (outcome.status === 'failed') {
			announce(outcome.failure.messageKey);
		}
	}

	const buttonPrimary =
		'min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 motion-safe:transition-colors dark:focus-visible:ring-offset-slate-900';
	const buttonSecondary =
		'min-h-11 rounded-md border border-slate-300 px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800';
	const fieldClass =
		'mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
	const labelClass = 'block text-sm font-medium text-slate-700 dark:text-slate-300';
	const dtClass = 'text-slate-500 dark:text-slate-400';
	const ddClass = 'text-slate-900 dark:text-slate-100';
</script>

<Card>
	<section aria-labelledby="flow-convert-heading" data-testid="flow-convert">
		<h2 id="flow-convert-heading" class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t('flow.bridge.convert.heading')}
		</h2>
		<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
			{$t('flow.bridge.convert.description')}
		</p>

		<p class="sr-only" role="status" aria-live="polite" data-testid="flow-convert-live">
			{liveKey ? $t(liveKey, { values: liveValues }) : ''}
		</p>

		{#if snap}
			{#if snap.sourceState === 'loading'}
				<p class="mt-6 text-sm text-slate-500 dark:text-slate-400">{$t('common.loading')}</p>
			{:else if snap.sourceState === 'unsupported'}
				<div
					class="mt-6 rounded-md border border-slate-200 p-6 text-center dark:border-slate-700"
					data-testid="flow-convert-unsupported"
				>
					<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">
						{$t('flow.bridge.convert.unsupported.title')}
					</h3>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{$t('flow.bridge.convert.unsupported.body')}
					</p>
				</div>
			{:else if snap.sourceState === 'not_found'}
				<div
					class="mt-6 rounded-md border border-slate-200 p-6 text-center dark:border-slate-700"
					data-testid="flow-convert-not-found"
				>
					<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">
						{$t('flow.bridge.convert.notFound.title')}
					</h3>
					<p class="mt-1 text-sm text-slate-600 dark:text-slate-300">
						{$t('flow.bridge.convert.notFound.body')}
					</p>
				</div>
			{:else if snap.sourceState === 'failed' && !snap.source}
				<div class="mt-6 space-y-3" role="alert">
					<p class="text-sm text-red-700 dark:text-red-300">
						{$t('flow.bridge.convert.error.sourceUnavailable')}
					</p>
					<button type="button" class={buttonSecondary} onclick={() => wizard.load()}>
						{$t('flow.bridge.convert.action.reload')}
					</button>
				</div>
			{:else if snap.source}
				<nav class="mt-4" aria-label={$t('flow.bridge.convert.stepsLabel')}>
					<ol class="flex flex-col gap-2 md:flex-row md:gap-1">
						{#each CONVERT_STEPS as step, index (step)}
							<li class="flex-1">
								<button
									type="button"
									class="flex min-h-11 w-full items-center gap-2 rounded-md border px-3 text-left text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:cursor-not-allowed {index ===
									stepIndex
										? 'border-blue-500 bg-blue-50 font-semibold text-blue-800 dark:bg-blue-950/40 dark:text-blue-200'
										: 'border-slate-200 text-slate-600 dark:border-slate-700 dark:text-slate-300'}"
									aria-current={index === stepIndex ? 'step' : undefined}
									disabled={index > stepIndex || snap.previewState === 'running' || snap.committing}
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
						data-testid="flow-convert-step"
					>
						{$t('flow.bridge.convert.stepOf', {
							values: {
								n: String(stepIndex + 1),
								total: String(CONVERT_STEPS.length),
								name: $t(STEP_KEYS[snap.step])
							}
						})}
					</h3>

					{#if snap.step === 'source'}
						<div class="mt-3 space-y-4" data-testid="flow-convert-source">
							<dl class="grid grid-cols-1 gap-3 text-sm md:grid-cols-3">
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.source.title')}</dt>
									<dd class={ddClass} data-testid="flow-convert-source-title">
										{snap.source.title}
									</dd>
								</div>
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.source.documentSeq')}</dt>
									<dd class={ddClass} data-testid="flow-convert-source-seq">
										{snap.source.document_seq}
									</dd>
								</div>
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.source.frontier')}</dt>
									<dd
										class="break-all font-mono text-xs {ddClass}"
										data-testid="flow-convert-source-frontier"
									>
										{snap.source.frontier}
									</dd>
								</div>
							</dl>
							<div
								class="rounded-md bg-slate-50 p-4 text-sm text-slate-700 dark:bg-slate-800 dark:text-slate-200"
							>
								<p class="font-medium">{$t('flow.bridge.convert.ownership.title')}</p>
								<ul class="mt-2 list-disc space-y-1 pl-5">
									<li>{$t('flow.bridge.convert.ownership.source')}</li>
									<li>{$t('flow.bridge.convert.ownership.target')}</li>
									<li>{$t('flow.bridge.convert.ownership.noSync')}</li>
								</ul>
							</div>
						</div>
					{:else if snap.step === 'mapping'}
						<div class="mt-3 space-y-4" data-testid="flow-convert-mapping">
							<div class="grid grid-cols-1 gap-4 md:grid-cols-2">
								<div>
									<label for="flow-convert-project" class={labelClass}>
										{$t('flow.bridge.convert.mapping.project')}
									</label>
									<select
										id="flow-convert-project"
										class={fieldClass}
										value={snap.projectId ?? ''}
										onchange={(event) => wizard.selectProject(event.currentTarget.value || null)}
									>
										<option value="">{$t('flow.bridge.convert.mapping.chooseProject')}</option>
										{#each snap.projects as project (project.id)}
											<option value={project.id}>{project.label}</option>
										{/each}
									</select>
									{#if snap.projects.length === 0}
										<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
											{$t('flow.bridge.convert.mapping.noProjects')}
										</p>
									{/if}
								</div>
								<div>
									<label for="flow-convert-form" class={labelClass}>
										{$t('flow.bridge.convert.mapping.form')}
									</label>
									<select
										id="flow-convert-form"
										class={fieldClass}
										value={snap.formId ?? ''}
										disabled={snap.formsState !== 'ready'}
										onchange={(event) => wizard.selectForm(event.currentTarget.value || null)}
									>
										<option value="">{$t('flow.bridge.convert.mapping.chooseForm')}</option>
										{#each snap.forms as form (form.id)}
											<option value={form.id}>{form.name}</option>
										{/each}
									</select>
									{#if snap.formsState === 'loading'}
										<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
											{$t('common.loading')}
										</p>
									{:else if snap.formsState === 'failed'}
										<p class="mt-1 text-xs text-red-700 dark:text-red-300" role="alert">
											{$t('flow.bridge.convert.mapping.formsFailed')}
										</p>
									{:else if snap.formsState === 'ready' && snap.forms.length === 0}
										<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
											{$t('flow.bridge.convert.mapping.noForms')}
										</p>
									{/if}
								</div>
							</div>

							{#if snap.formId}
								<div>
									<label for="flow-convert-title" class={labelClass}>
										{$t('flow.bridge.convert.mapping.recordTitle')}
									</label>
									<input
										id="flow-convert-title"
										type="text"
										class={fieldClass}
										value={snap.title}
										oninput={(event) => wizard.setTitle(event.currentTarget.value)}
										aria-describedby="flow-convert-title-hint"
									/>
									<p
										id="flow-convert-title-hint"
										class="mt-1 text-xs text-slate-500 dark:text-slate-400"
									>
										{$t('flow.bridge.convert.mapping.recordTitleHint')}
									</p>
								</div>

								<p class="text-sm text-slate-600 dark:text-slate-300">
									{$t('flow.bridge.convert.mapping.valuesHint')}
								</p>

								{#if snap.fields.length === 0}
									<p class="text-sm text-slate-500 dark:text-slate-400">
										{$t('flow.bridge.convert.mapping.noFields')}
									</p>
								{:else}
									<ul class="space-y-4" data-testid="flow-convert-fields">
										{#each snap.fields as field (field.key)}
											{@const problem = snap.problems[field.key]}
											{@const inputId = `flow-convert-field-${field.key}`}
											{@const hintId = `${inputId}-hint`}
											<li data-field-key={field.key}>
												{#if field.disposition.kind === 'input'}
													{@const kind = field.disposition.input}
													{#if kind === 'multi_select'}
														<fieldset aria-describedby={problem ? hintId : undefined}>
															<legend class={labelClass}>
																{field.label}{field.required ? ' *' : ''}
															</legend>
															<div class="mt-1 flex flex-wrap gap-3">
																{#each field.options as option (option)}
																	<label
																		class="flex min-h-11 items-center gap-2 text-sm text-slate-700 dark:text-slate-200"
																	>
																		<input
																			type="checkbox"
																			class="h-5 w-5"
																			checked={selectedOptions(field).includes(option)}
																			onchange={(event) =>
																				toggleOption(field, option, event.currentTarget.checked)}
																		/>
																		{option}
																	</label>
																{/each}
															</div>
														</fieldset>
													{:else}
														<label for={inputId} class={labelClass}>
															{field.label}{field.required ? ' *' : ''}
														</label>
														{#if kind === 'single_select' || kind === 'boolean'}
															<select
																id={inputId}
																class={fieldClass}
																value={inputValue(field)}
																aria-invalid={problem ? 'true' : undefined}
																aria-describedby={problem ? hintId : undefined}
																onchange={(event) =>
																	wizard.setValue(field.key, event.currentTarget.value)}
															>
																<option value="">{$t('flow.bridge.convert.mapping.unset')}</option>
																{#if kind === 'boolean'}
																	<option value="true">{$t('flow.bridge.convert.value.yes')}</option
																	>
																	<option value="false">{$t('flow.bridge.convert.value.no')}</option
																	>
																{:else}
																	{#each field.options as option (option)}
																		<option value={option}>{option}</option>
																	{/each}
																{/if}
															</select>
														{:else if kind === 'long_text'}
															<textarea
																id={inputId}
																rows="3"
																class="{fieldClass} py-2"
																value={inputValue(field)}
																aria-invalid={problem ? 'true' : undefined}
																aria-describedby={problem ? hintId : undefined}
																oninput={(event) =>
																	wizard.setValue(field.key, event.currentTarget.value)}
															></textarea>
														{:else}
															<input
																id={inputId}
																type={kind === 'email'
																	? 'email'
																	: kind === 'date'
																		? 'date'
																		: kind === 'datetime'
																			? 'datetime-local'
																			: 'text'}
																inputmode={kind === 'integer' ||
																kind === 'rating' ||
																kind === 'progress'
																	? 'numeric'
																	: kind === 'decimal'
																		? 'decimal'
																		: undefined}
																class={fieldClass}
																value={inputValue(field)}
																aria-invalid={problem ? 'true' : undefined}
																aria-describedby={problem ? hintId : undefined}
																oninput={(event) =>
																	wizard.setValue(field.key, event.currentTarget.value)}
															/>
														{/if}
													{/if}
												{:else}
													<p class={labelClass}>{field.label}{field.required ? ' *' : ''}</p>
													<p class="mt-1 text-xs text-slate-500 dark:text-slate-400">
														{field.disposition.kind === 'server'
															? $t('flow.bridge.convert.mapping.serverFilled')
															: $t('flow.bridge.convert.mapping.unsupportedField')}
													</p>
												{/if}
												{#if problem}
													<p id={hintId} class="mt-1 text-xs text-red-700 dark:text-red-300">
														{$t(FIELD_PROBLEM_KEYS[problem])}
													</p>
												{/if}
											</li>
										{/each}
									</ul>
								{/if}
							{/if}
						</div>
					{:else if snap.step === 'preview'}
						<div class="mt-3 space-y-4" data-testid="flow-convert-preview">
							{#if snap.previewState === 'running' || snap.refreshingSource}
								<p class="text-sm text-slate-600 dark:text-slate-300">
									{$t('flow.bridge.convert.preview.running')}
								</p>
							{/if}

							{#if snap.previewFailure}
								<div
									class="space-y-3 rounded-md border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40"
									role="alert"
									data-testid="flow-convert-preview-error"
									data-kind={snap.previewFailure.kind}
								>
									<p class="text-sm text-red-800 dark:text-red-200">
										{$t(snap.previewFailure.messageKey)}
									</p>
									{#if snap.previewFailure.permissionState}
										<ul class="list-disc space-y-1 pl-5 text-sm text-red-800 dark:text-red-200">
											{#each permissionExplanation(snap.previewFailure.permissionState) as key (key)}
												<li>{$t(key)}</li>
											{/each}
										</ul>
									{/if}
									{#if snap.previewFailure.kind === 'stale_frontier'}
										<button
											type="button"
											class={buttonSecondary}
											disabled={snap.refreshingSource}
											onclick={refreshAndPreview}
										>
											{$t('flow.bridge.convert.action.refreshSource')}
										</button>
									{:else if snap.previewFailure.retryable}
										<button type="button" class={buttonSecondary} onclick={rePreview}>
											{$t('flow.bridge.convert.action.retryPreview')}
										</button>
									{/if}
								</div>
							{/if}

							{#if snap.preview}
								{#if snap.expired}
									<div
										class="space-y-3 rounded-md border border-amber-200 bg-amber-50 p-4 dark:border-amber-900 dark:bg-amber-950/40"
										role="alert"
										data-testid="flow-convert-expired"
									>
										<p class="text-sm text-amber-900 dark:text-amber-100">
											{$t('flow.bridge.convert.preview.expired')}
										</p>
										<button type="button" class={buttonSecondary} onclick={rePreview}>
											{$t('flow.bridge.convert.action.previewAgain')}
										</button>
									</div>
								{/if}
								<p class="text-sm text-slate-600 dark:text-slate-300">
									{$t('flow.bridge.convert.preview.notCommitted')}
								</p>
								<dl class="grid grid-cols-1 gap-3 text-sm md:grid-cols-2">
									<div>
										<dt class={dtClass}>{$t('flow.bridge.convert.preview.schemaVersion')}</dt>
										<dd class={ddClass} data-testid="flow-convert-schema-version">
											{snap.preview.target_schema_version}
										</dd>
									</div>
									<div>
										<dt class={dtClass}>{$t('flow.bridge.convert.preview.estimatedObjects')}</dt>
										<dd class={ddClass} data-testid="flow-convert-estimated">
											{snap.preview.estimated_objects}
										</dd>
									</div>
									<div>
										<dt class={dtClass}>{$t('flow.bridge.convert.preview.expiresAt')}</dt>
										<dd class={ddClass}>
											{formatTime(snap.preview.expires_at)}
											{#if !snap.expired && secondsLeft !== null}
												<span data-testid="flow-convert-countdown">
													({$t('flow.bridge.convert.preview.countdown', {
														values: { time: formatCountdown(secondsLeft) }
													})})
												</span>
											{/if}
										</dd>
									</div>
									<div>
										<dt class={dtClass}>{$t('flow.bridge.convert.preview.sourceFrontier')}</dt>
										<dd class="break-all font-mono text-xs {ddClass}">
											{snap.preview.source_frontier}
										</dd>
									</div>
								</dl>

								<div>
									<h4 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
										{$t('flow.bridge.convert.preview.permission')}
									</h4>
									<ul
										class="mt-2 list-disc space-y-1 pl-5 text-sm text-slate-700 dark:text-slate-200"
										data-testid="flow-convert-permission"
									>
										{#each permissionExplanation(snap.preview.permission_decision) as key (key)}
											<li>{$t(key)}</li>
										{/each}
									</ul>
								</div>

								<div>
									<h4 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
										{$t('flow.bridge.convert.preview.mapping')}
									</h4>
									<dl class="mt-2 space-y-1 text-sm">
										<div class="flex flex-col gap-1 sm:flex-row sm:gap-2">
											<dt class={dtClass}>{$t('flow.bridge.convert.mapping.recordTitle')}</dt>
											<dd class={ddClass}>
												{typeof snap.preview.mapping.title === 'string'
													? snap.preview.mapping.title
													: $t('flow.bridge.convert.preview.titleFromPage')}
											</dd>
										</div>
										{#each previewValues(snap.preview.mapping) as [key, value] (key)}
											<div class="flex flex-col gap-1 sm:flex-row sm:gap-2">
												<dt class={dtClass}>{fieldLabel(key)}</dt>
												<dd class={ddClass}>{previewValue(value)}</dd>
											</div>
										{/each}
									</dl>
								</div>

								<div>
									<h4 class="text-sm font-semibold text-slate-900 dark:text-slate-100">
										{$t('flow.bridge.convert.preview.warnings')}
									</h4>
									{#if snap.preview.warnings.length === 0}
										<p class="mt-1 text-sm text-slate-500 dark:text-slate-400">
											{$t('flow.bridge.convert.preview.noWarnings')}
										</p>
									{:else}
										<ul
											class="mt-1 list-disc space-y-1 pl-5 text-sm text-amber-800 dark:text-amber-200"
										>
											{#each snap.preview.warnings as warning, index (index)}
												<li>{warning}</li>
											{/each}
										</ul>
									{/if}
								</div>
							{/if}
						</div>
					{:else if snap.step === 'commit' && snap.preview}
						<div class="mt-3 space-y-4" data-testid="flow-convert-commit">
							<dl class="grid grid-cols-1 gap-3 text-sm md:grid-cols-2">
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.commit.source')}</dt>
									<dd class={ddClass}>{snap.source.title}</dd>
								</div>
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.commit.target')}</dt>
									<dd class={ddClass}>
										{selectedProject ? `${selectedProject.label} / ` : ''}{selectedForm?.name ?? ''}
									</dd>
								</div>
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.preview.schemaVersion')}</dt>
									<dd class={ddClass} data-testid="flow-convert-commit-schema-version">
										{snap.preview.target_schema_version}
									</dd>
								</div>
								<div>
									<dt class={dtClass}>{$t('flow.bridge.convert.preview.estimatedObjects')}</dt>
									<dd class={ddClass}>{snap.preview.estimated_objects}</dd>
								</div>
								<div class="md:col-span-2">
									<dt class={dtClass}>{$t('flow.bridge.convert.preview.sourceFrontier')}</dt>
									<dd class="break-all font-mono text-xs {ddClass}">
										{snap.preview.source_frontier}
									</dd>
								</div>
							</dl>

							{#if snap.expired}
								<div
									class="space-y-3 rounded-md border border-amber-200 bg-amber-50 p-4 dark:border-amber-900 dark:bg-amber-950/40"
									role="alert"
									data-testid="flow-convert-commit-expired"
								>
									<p class="text-sm text-amber-900 dark:text-amber-100">
										{$t('flow.bridge.convert.commit.expired')}
									</p>
									<button
										type="button"
										class={buttonSecondary}
										onclick={() => wizard.goTo('preview')}
									>
										{$t('flow.bridge.convert.action.backToPreview')}
									</button>
								</div>
							{:else if secondsLeft !== null}
								<p class="text-sm text-slate-600 dark:text-slate-300">
									{$t('flow.bridge.convert.preview.countdown', {
										values: { time: formatCountdown(secondsLeft) }
									})}
								</p>
							{/if}

							<label
								class="flex min-h-11 items-start gap-3 text-sm text-slate-800 dark:text-slate-100"
							>
								<input
									type="checkbox"
									class="mt-0.5 h-5 w-5"
									checked={snap.acknowledged}
									disabled={snap.committing || snap.expired}
									onchange={(event) => wizard.acknowledge(event.currentTarget.checked)}
									data-testid="flow-convert-acknowledge"
								/>
								<span>{$t('flow.bridge.convert.commit.acknowledge')}</span>
							</label>

							{#if snap.commitFailure}
								<div
									class="space-y-3 rounded-md border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40"
									role="alert"
									data-testid="flow-convert-commit-error"
									data-kind={snap.commitFailure.kind}
								>
									<p class="text-sm text-red-800 dark:text-red-200">
										{$t(snap.commitFailure.messageKey)}
									</p>
									{#if snap.commitFailure.permissionState}
										<ul class="list-disc space-y-1 pl-5 text-sm text-red-800 dark:text-red-200">
											{#each permissionExplanation(snap.commitFailure.permissionState) as key (key)}
												<li>{$t(key)}</li>
											{/each}
										</ul>
									{/if}
									{#if snap.commitFailure.kind === 'stale_frontier'}
										<button
											type="button"
											class={buttonSecondary}
											disabled={snap.refreshingSource}
											onclick={refreshAndPreview}
										>
											{$t('flow.bridge.convert.action.refreshSource')}
										</button>
									{:else if !snap.commitFailure.retryable}
										<button
											type="button"
											class={buttonSecondary}
											onclick={() => wizard.goTo('mapping')}
										>
											{$t('flow.bridge.convert.action.backToMapping')}
										</button>
									{/if}
								</div>
							{/if}

							<button
								type="button"
								class={buttonPrimary}
								disabled={!snap.canCommit}
								onclick={commit}
								data-testid="flow-convert-commit-button"
							>
								{snap.committing
									? $t('flow.bridge.convert.commit.committing')
									: snap.commitFailure?.retryable
										? $t('flow.bridge.convert.commit.retry')
										: $t('flow.bridge.convert.commit.submit')}
							</button>
						</div>
					{/if}
				</div>

				<div
					class="mt-6 flex flex-col-reverse gap-3 border-t border-slate-200 pt-4 sm:flex-row sm:justify-between dark:border-slate-700"
				>
					<button
						type="button"
						class={buttonSecondary}
						disabled={stepIndex === 0 || snap.previewState === 'running' || snap.committing}
						onclick={() => wizard.back()}
					>
						{$t('flow.bridge.convert.action.back')}
					</button>
					{#if snap.step !== 'commit'}
						<button
							type="button"
							class={buttonPrimary}
							disabled={!snap.canAdvance || snap.previewState === 'running'}
							onclick={next}
							data-testid="flow-convert-next"
						>
							{snap.step === 'mapping'
								? $t('flow.bridge.convert.action.runPreview')
								: snap.step === 'preview'
									? $t('flow.bridge.convert.action.toCommit')
									: $t('flow.bridge.convert.action.next')}
						</button>
					{/if}
				</div>
			{/if}
		{/if}
	</section>
</Card>
