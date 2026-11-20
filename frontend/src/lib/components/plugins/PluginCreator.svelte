<script lang="ts">
	import { onMount, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		legacyDraftKey,
		creatorTemplates,
		draftStorageKey,
		initialTemplate,
		originSlot,
		type CreatorTemplateId
	} from '#lib/plugins/creator/registry.js';
	import { personalDraftKey, personalLegacyKey } from '#lib/plugins/creator/personal-drafts.js';
	import Button from '#lib/components/Button.svelte';
	import CreatorPanelPreview from '#lib/components/plugins/CreatorPanelPreview.svelte';
	import { apiClient } from '#lib/api/client.js';
	import { formsApi } from '#lib/api/forms.js';
	import { projectsApi } from '#lib/api/projects.js';
	import {
		isV3,
		pluginLibraryApi,
		pluginPackagesApi,
		type PluginSlot,
		type PluginSlotV3,
		type PluginUpload,
		type PluginUploadV3
	} from '#lib/api/plugin-packages.js';
	import {
		actionSlot,
		buildCreatorPackage,
		creatorSlot,
		detailSlot,
		invalidDraftFields,
		type CreatorDraft
	} from '#lib/plugins/creator/template.js';
	import {
		buildUiViewPackage,
		invalidUiViewFields,
		uiViewSlots,
		type SampleField,
		type UiViewContributionDraft,
		type UiViewDraft
	} from '#lib/plugins/creator/ui-view-template.js';
	import { slotLabelKey } from '#lib/plugins/layout-v3.js';

	type CreatorScope =
		| { kind: 'personal'; draftId: string }
		| { kind: 'project'; workspaceId: string; projectId: string };
	interface Bundle {
		upload: PluginUpload | PluginUploadV3;
		html: string;
		json: string;
		fixtureCents?: number;
	}

	let {
		context,
		initialSlot,
		formKey,
		initialTemplateId = null
	}: {
		context: CreatorScope;
		initialSlot: string | null;
		formKey: string | null;
		initialTemplateId?: string | null;
	} = $props();

	const personal = $derived(context.kind === 'personal');
	const origin = $derived(originSlot(initialSlot));
	const recordOrigin = $derived(creatorSlot(initialSlot));
	const settingsUrl = $derived(
		context.kind === 'project'
			? `/workspace/${context.workspaceId}/projects/${context.projectId}/settings/plugins`
			: '/plugins'
	);
	let templateId = $state<CreatorTemplateId>(
		initialTemplate(
			untrack(() => initialTemplateId),
			untrack(() => origin)
		)
	);
	let orderDraft = $state<CreatorDraft>(
		creatorTemplates.order_amount.defaults({ slot: 'form.record.detail', formKey: null })
	);
	let uiDraft = $state<UiViewDraft>(
		creatorTemplates.ui_view.defaults({ slot: 'project.view', formKey: null })
	);
	let allowed = $state(false);
	let layoutSupported = $state(true);
	let extensionSupported = $state(true);
	let checking = $state(true);
	let building = $state(false);
	let installing = $state(false);
	let error = $state('');
	let backendMessage = $state('');
	let installed = $state(false);
	let bundle = $state.raw<Bundle | null>(null);
	let previewSlot = $state<PluginSlot>(detailSlot);
	let previewContribution = $state('');
	let previewSchema = $state.raw<SampleField[] | null>(null);
	let schemaFailed = $state(false);
	let sampleRan = $state(false);
	let reviewed = $state(false);
	let reviewStart: HTMLHeadingElement | undefined = $state();
	let storageKey = '';
	let actorId = '';
	let scope = 0;
	let revision = $state(0);

	const isUi = $derived(templateId === 'ui_view');
	const template = $derived(creatorTemplates[templateId]);
	const invalid = $derived(isUi ? invalidUiViewFields(uiDraft) : invalidDraftFields(orderDraft));
	const templateAvailable = $derived(isUi ? extensionSupported : layoutSupported);
	const details = ['detailLabel', 'detailTitle', 'detailText'] as const;
	const actions = ['actionLabel', 'actionTitle', 'actionText'] as const;
	const metadata = ['key', 'name', 'publisher', 'version', 'formKey'] as const;
	const uiMetadata = ['key', 'name', 'publisher', 'version'] as const;
	const uiContent = ['title', 'text'] as const;
	const review = $derived(bundle && isV3(bundle.upload.manifest) ? bundle.upload.manifest : null);

	$effect(() => {
		const mine = ++scope;
		const current = context;
		const actor = apiClient.getAuthPrincipal();
		allowed = false;
		checking = true;
		resetBuild();
		previewSlot = recordOrigin;
		storageKey = '';
		actorId = actor ?? '';
		if (!actor) {
			checking = false;
			error = 'plugins.manageDenied';
		} else if (current.kind === 'personal') {
			untrack(() => {
				allowed = true;
				checking = false;
			});
		} else if (current.workspaceId && current.projectId) {
			void authorizeProject(current, mine);
		} else {
			checking = false;
			error = 'plugins.manageDenied';
		}
	});

	$effect(() => {
		const id = templateId;
		const ok = allowed;
		const slot = origin;
		const form = formKey;
		if (ok) untrack(() => setupDraft(id, slot, form));
	});

	onMount(() => {
		const auth = apiClient.authSessionSignal();
		const aborted = () => {
			scope++;
			storageKey = '';
			allowed = false;
			checking = false;
			resetBuild();
			error = 'plugins.sessionChanged';
		};
		auth.addEventListener('abort', aborted, { once: true });
		return () => {
			scope++;
			auth.removeEventListener('abort', aborted);
		};
	});

	async function authorizeProject(
		project: Extract<CreatorScope, { kind: 'project' }>,
		mine: number
	) {
		const [metadata, catalog] = await Promise.all([
			projectsApi.get(project.projectId),
			pluginPackagesApi.list(project.projectId)
		]);
		if (scope !== mine) return;
		layoutSupported = catalog.data?.layout_supported === true;
		extensionSupported = catalog.data?.extension_supported === true;
		if (
			metadata.data?.workspace_id !== project.workspaceId ||
			catalog.code !== 0 ||
			(!layoutSupported && !extensionSupported)
		) {
			checking = false;
			error = catalog.code === 403 ? 'plugins.manageDenied' : 'plugins.creator.unavailable';
			return;
		}
		allowed = true;
		checking = false;
	}

	function draftKeys(id: CreatorTemplateId, slot: PluginSlotV3, form: string | null) {
		if (context.kind === 'personal')
			return {
				key: personalDraftKey(actorId, context.draftId, id),
				legacy: id === 'order_amount' ? personalLegacyKey(actorId, context.draftId) : null
			};
		const target = `${encodeURIComponent(context.projectId)}:${encodeURIComponent(form ?? slot)}`;
		return {
			key: draftStorageKey('project', actorId, id, target),
			legacy:
				id === 'order_amount'
					? legacyDraftKey('project', actorId, `${context.projectId}:${form ?? 'order_line'}`)
					: null
		};
	}

	function setupDraft(id: CreatorTemplateId, slot: PluginSlotV3, form: string | null) {
		resetBuild();
		previewSlot = creatorSlot(slot);
		const keys = draftKeys(id, slot, form);
		storageKey = keys.key;
		let prior: string | null = null;
		try {
			prior =
				localStorage.getItem(keys.key) ?? (keys.legacy ? localStorage.getItem(keys.legacy) : null);
		} catch {
			/* The draft remains usable in memory. */
		}
		if (id === 'ui_view') uiDraft = creatorTemplates.ui_view.read(prior, { slot, formKey: form });
		else orderDraft = creatorTemplates.order_amount.read(prior, { slot, formKey: form });
		if (!prior && personal) storeDraft();
	}

	function storeDraft() {
		try {
			if (storageKey) localStorage.setItem(storageKey, JSON.stringify(isUi ? uiDraft : orderDraft));
		} catch {
			/* Browser storage can be disabled. */
		}
	}

	function resetBuild() {
		bundle = null;
		previewSchema = null;
		schemaFailed = false;
		reviewed = sampleRan = building = installing = installed = false;
		error = backendMessage = '';
	}

	function changed() {
		revision++;
		resetBuild();
		storeDraft();
	}

	function edit(field: keyof CreatorDraft, value: string) {
		orderDraft = { ...orderDraft, [field]: value };
		changed();
	}

	function editUi(patch: Partial<UiViewDraft>) {
		uiDraft = { ...uiDraft, ...patch };
		changed();
	}

	function editLocation(slot: string, patch: Partial<UiViewContributionDraft>) {
		editUi({
			contributions: uiDraft.contributions.map((item) =>
				item.slot === slot ? { ...item, ...patch } : item
			)
		});
	}

	function chooseTemplate(id: CreatorTemplateId) {
		if (id === templateId || building || installing) return;
		templateId = id;
	}

	async function loadSchema(): Promise<SampleField[] | null> {
		if (context.kind !== 'project') return null;
		const result = await formsApi.list(context.projectId, { per_page: 100 });
		const form = result.data?.items.find((item) => item.key === uiDraft.readFormKey.trim());
		const fields = form && Array.isArray(form.schema.fields) ? form.schema.fields : null;
		if (!fields) return null;
		return fields.flatMap((field: unknown) => {
			const item = field as { key?: unknown; label?: unknown; type?: unknown; options?: unknown };
			return typeof item.key === 'string'
				? [
						{
							key: item.key,
							label: typeof item.label === 'string' ? item.label : item.key,
							type: typeof item.type === 'string' ? item.type : 'text',
							...(Array.isArray(item.options)
								? { options: item.options.filter((value) => typeof value === 'string') }
								: {})
						}
					]
				: [];
		});
	}

	async function build() {
		if (!allowed || !templateAvailable || invalid.length || building || installing) return;
		const buildRevision = ++revision;
		const mine = scope;
		resetBuild();
		building = true;
		try {
			const output: Bundle = isUi
				? await buildUiViewPackage($state.snapshot(uiDraft))
				: await buildCreatorPackage($state.snapshot(orderDraft));
			let schema: SampleField[] | null = null;
			if (isUi) {
				try {
					schema = await loadSchema();
				} catch {
					schema = null;
				}
			}
			if (buildRevision !== revision || mine !== scope) return;
			bundle = output;
			previewSchema = schema;
			schemaFailed = isUi && !personal && !schema;
			previewContribution = isV3(output.upload.manifest)
				? (output.upload.manifest.contributions[0]?.id ?? '')
				: '';
			await tick();
			reviewStart?.focus();
		} catch {
			if (buildRevision === revision && mine === scope) error = 'plugins.creator.buildFailed';
		} finally {
			if (buildRevision === revision && mine === scope) building = false;
		}
	}

	function download() {
		if (!bundle || !reviewed || !sampleRan) return;
		const url = URL.createObjectURL(new Blob([bundle.json], { type: 'application/json' }));
		const link = document.createElement('a');
		link.href = url;
		link.download = `${bundle.upload.manifest.key}-${bundle.upload.manifest.version}.json`;
		link.click();
		setTimeout(() => URL.revokeObjectURL(url), 0);
	}

	async function install() {
		if (!bundle || !reviewed || !sampleRan || installing) return;
		const mine = scope;
		installing = true;
		error = backendMessage = '';
		const result = await (context.kind === 'personal'
			? pluginLibraryApi.save(bundle.upload)
			: pluginPackagesApi.install(context.projectId, bundle.upload));
		if (mine !== scope) return;
		installing = false;
		installed = result.code === 0 && !!result.data;
		if (!installed) {
			error = personal ? 'plugins.library.saveFailed' : 'plugins.installFailed';
			backendMessage = result.message;
		}
	}
</script>

{#snippet fieldEditor(field: keyof CreatorDraft)}
	<label class="flex flex-col gap-1 text-sm">
		<span class="font-medium">{$t(`plugins.creator.field.${field}`)}</span>
		<input
			class="rounded-control border border-input bg-transparent px-3 py-2"
			value={orderDraft[field]}
			maxlength={field.endsWith('Text') ? 320 : 120}
			required
			disabled={building || installing}
			aria-invalid={invalid.includes(field)}
			aria-describedby="creator-field-help"
			oninput={(event) => edit(field, event.currentTarget.value)}
		/>
	</label>
{/snippet}

{#snippet uiInput(
	field: string,
	value: string,
	change: (value: string) => void,
	max = 120,
	invalidName = field
)}
	<label class="flex flex-col gap-1 text-sm">
		<span class="font-medium">{$t(`plugins.creatorV3.field.${field}`)}</span>
		<input
			class="rounded-control border border-input bg-transparent px-3 py-2"
			{value}
			maxlength={max}
			disabled={building || installing}
			aria-invalid={invalid.includes(invalidName)}
			aria-describedby="creator-field-help"
			oninput={(event) => change(event.currentTarget.value)}
		/>
	</label>
{/snippet}

<div class="mx-auto max-w-5xl space-y-5 px-1">
	<header class="space-y-2">
		<a href={settingsUrl} class="text-sm text-fg underline"
			>{$t(personal ? 'plugins.library.title' : 'plugins.projectSettings')}</a
		>
		<h1 class="text-2xl font-semibold">{$t('plugins.creator.title')}</h1>
		<p class="text-sm text-fg-muted">
			{$t(isUi ? 'plugins.creatorV3.templateHelp' : 'plugins.creator.template')}
		</p>
	</header>
	{#if error}
		<p role="alert" class="rounded-control border border-danger px-3 py-2 text-sm text-danger">
			{$t(error)}
			{backendMessage}
		</p>
	{/if}
	{#if checking}<p role="status" class="text-sm">{$t('common.loading')}</p>{/if}
	{#if allowed}
		<section class="space-y-4 rounded-card border border-line bg-raised p-4">
			<div class="flex flex-wrap items-center justify-between gap-3">
				<h2 class="font-semibold">{$t('plugins.creator.draft')}</h2>
				<label class="flex items-center gap-2 text-sm">
					<span>{$t('plugins.creatorV3.template')}</span>
					<select
						value={templateId}
						disabled={building || installing}
						onchange={(event) => chooseTemplate(event.currentTarget.value as CreatorTemplateId)}
						class="rounded-control border border-input bg-transparent px-3 py-1"
					>
						{#each Object.keys(creatorTemplates) as id (id)}
							<option value={id}>{$t(`plugins.creatorV3.templateName.${id}`)}</option>
						{/each}
					</select>
				</label>
			</div>
			<p class="text-sm text-fg-muted">
				{$t('plugins.creator.origin', { values: { slot: $t(slotLabelKey(origin)) } })}
				{$t(personal ? 'plugins.library.localDraft' : 'plugins.creator.localDraft')}
			</p>
			{#if !templateAvailable}
				<p role="status" class="text-sm">
					{$t(isUi ? 'plugins.creatorV3.extensionsUnavailable' : 'plugins.creator.unavailable')}
				</p>
			{/if}
			<form
				class="space-y-4"
				onsubmit={(event) => {
					event.preventDefault();
					void build();
				}}
			>
				{#if isUi}
					<fieldset class="grid gap-3 sm:grid-cols-2">
						<legend class="pb-2 text-sm font-semibold">{$t('plugins.creator.package')}</legend>
						{#each uiMetadata as field (field)}
							{@render uiInput(field, uiDraft[field], (value) => editUi({ [field]: value }))}
						{/each}
					</fieldset>
					<p id="creator-field-help" class="text-xs text-fg-muted">
						{$t('plugins.creatorV3.fieldHelp')}
					</p>
					<p class="text-sm">{$t('plugins.creatorV3.uiIntro')}</p>
					<div class="grid gap-3 sm:grid-cols-2">
						{#each uiViewSlots as slot (slot)}
							{@const item = uiDraft.contributions.find((entry) => entry.slot === slot)}
							{#if item}
								<fieldset class="space-y-2 rounded-card border border-line p-3">
									<legend class="px-1 text-sm font-medium">{$t(slotLabelKey(slot))}</legend>
									<label class="flex items-start gap-2 text-sm">
										<input
											type="checkbox"
											class="mt-1"
											checked={item.enabled}
											disabled={building || installing}
											onchange={(event) =>
												editLocation(slot, { enabled: event.currentTarget.checked })}
										/>
										<span
											>{$t('plugins.creatorV3.locationEnabled', {
												values: { slot: $t(slotLabelKey(slot)) }
											})}</span
										>
									</label>
									{#if item.enabled}
										{@render uiInput(
											'label',
											item.label,
											(value) => editLocation(slot, { label: value }),
											120,
											`contributions.${slot}.label`
										)}
										{#if slot === 'project.view'}
											{@render uiInput(
												'route',
												item.route,
												(value) => editLocation(slot, { route: value }),
												32,
												`contributions.${slot}.route`
											)}
										{:else}
											{@render uiInput(
												'formKey',
												item.formKey,
												(value) => editLocation(slot, { formKey: value }),
												64,
												`contributions.${slot}.formKey`
											)}
										{/if}
									{/if}
								</fieldset>
							{/if}
						{/each}
					</div>
					{#if invalid.includes('contributions')}<p role="status" class="text-sm">
							{$t('plugins.creatorV3.needLocation')}
						</p>{/if}
					<fieldset class="grid gap-3 rounded-card border border-line p-3 sm:grid-cols-2">
						<legend class="px-1 text-sm font-medium">{$t('plugins.creatorV3.reads')}</legend>
						{@render uiInput(
							'readFormKey',
							uiDraft.readFormKey,
							(value) => editUi({ readFormKey: value }),
							64
						)}
						{@render uiInput(
							'readFields',
							uiDraft.readFields,
							(value) => editUi({ readFields: value }),
							640
						)}
						<p class="text-xs text-fg-muted sm:col-span-2">
							{$t('plugins.creatorV3.readHelp')}
						</p>
						<label class="flex flex-col gap-1 text-sm">
							<span class="font-medium">{$t('plugins.creatorV3.presentationLabel')}</span>
							<select
								value={uiDraft.presentation}
								disabled={building || installing}
								onchange={(event) =>
									editUi({
										presentation: event.currentTarget.value === 'summary' ? 'summary' : 'table'
									})}
								class="rounded-control border border-input bg-transparent px-3 py-2"
							>
								<option value="table">{$t('plugins.creatorV3.presentation.table')}</option>
								<option value="summary">{$t('plugins.creatorV3.presentation.summary')}</option>
							</select>
						</label>
						{#each uiContent as field (field)}
							{@render uiInput(
								field,
								uiDraft[field],
								(value) => editUi({ [field]: value }),
								field === 'text' ? 320 : 120
							)}
						{/each}
					</fieldset>
				{:else}
					<fieldset class="grid gap-3 sm:grid-cols-2">
						<legend class="pb-2 text-sm font-semibold">{$t('plugins.creator.package')}</legend>
						{#each metadata as field (field)}{@render fieldEditor(field)}{/each}
					</fieldset>
					<p id="creator-field-help" class="text-xs text-fg-muted">
						{$t('plugins.creator.fieldHelp')}
					</p>
					<div class="grid gap-3 sm:grid-cols-2">
						{#each recordOrigin === actionSlot ? [actionSlot, detailSlot] : [detailSlot, actionSlot] as slot (slot)}
							<fieldset class="space-y-2 rounded-card border border-line p-3">
								<legend class="px-1 text-sm font-medium">{$t(`plugins.slot.${slot}`)}</legend>
								{#each slot === actionSlot ? actions : details as field (field)}{@render fieldEditor(
										field
									)}{/each}
							</fieldset>
						{/each}
					</div>
				{/if}
				{#if invalid.length}<p role="status" class="text-sm">
						{$t('plugins.creator.invalidDraft')}
					</p>{/if}
				<Button
					type="submit"
					loading={building}
					disabled={invalid.length > 0 || installing || !templateAvailable}
					>{$t('plugins.creator.build')}</Button
				>
			</form>
		</section>
		{#if bundle}
			<section class="space-y-3 rounded-card border border-line bg-raised p-4">
				<h2 class="font-semibold" tabindex="-1" bind:this={reviewStart}>
					{$t('plugins.creator.review')}
				</h2>
				{#if review}
					<p class="text-sm">
						{$t(
							personal
								? 'plugins.creatorV3.previewSamplePersonal'
								: 'plugins.creatorV3.previewSampleProject'
						)}
					</p>
					{#if schemaFailed}<p role="status" class="text-sm">
							{$t('plugins.creatorV3.previewSchemaFailed')}
						</p>{/if}
					<label class="flex flex-wrap items-center gap-2 text-sm">
						{$t('plugins.creatorV3.slotChoice')}
						<select
							bind:value={previewContribution}
							class="rounded-control border border-input bg-transparent px-3 py-1"
						>
							{#each review.contributions as item (item.id)}
								<option value={item.id}>{item.label} · {$t(slotLabelKey(item.slot))}</option>
							{/each}
						</select>
					</label>
					{#key `${revision}:${previewContribution}`}
						{@const picked = review.contributions.find((item) => item.id === previewContribution)}
						{#if picked}
							<CreatorPanelPreview
								html={bundle.html}
								slot={picked.slot}
								v3={{
									manifest: review,
									contributionId: picked.id,
									webSha256: review.entry.sha256,
									schema: previewSchema,
									personal,
									onViewed: () => {
										sampleRan = true;
									}
								}}
							/>
						{/if}
					{/key}
				{:else}
					<p class="text-sm">{$t('plugins.creator.previewRecord')}</p>
					<label class="flex flex-wrap items-center gap-2 text-sm">
						{$t('plugins.creator.previewLocation')}
						<select
							bind:value={previewSlot}
							class="rounded-control border border-input bg-transparent px-3 py-1"
						>
							<option value={detailSlot}>{$t(`plugins.slot.${detailSlot}`)}</option>
							<option value={actionSlot}>{$t(`plugins.slot.${actionSlot}`)}</option>
						</select>
					</label>
					{#key `${revision}:${previewSlot}`}
						<CreatorPanelPreview
							html={bundle.html}
							slot={previewSlot}
							onPreview={() => {
								sampleRan = true;
								return bundle?.fixtureCents ?? null;
							}}
						/>
					{/key}
				{/if}
				<div class="space-y-2 border-t border-line pt-3 text-sm">
					<h3 class="font-medium">
						{bundle.upload.manifest.name} · v{bundle.upload.manifest.version}
					</h3>
					{#if review}
						<h4 class="font-medium">{$t('plugins.creatorV3.reviewLocations')}</h4>
						<ul class="list-disc pl-5">
							{#each review.contributions as item (item.id)}
								<li>
									{$t('plugins.creatorV3.reviewLocation', {
										values: { label: item.label, slot: $t(slotLabelKey(item.slot)) }
									})}
									· {item.route
										? $t('plugins.creatorV3.reviewRoute', { values: { route: item.route } })
										: $t('plugins.creatorV3.reviewForm', { values: { form: item.form_key } })}
								</li>
							{/each}
						</ul>
						{#each review.reads as read (read.id)}
							<p>
								{$t('plugins.creatorV3.reviewReads', {
									values: { fields: read.fields.join(', '), form: read.form_key }
								})}
							</p>
						{:else}
							<p>{$t('plugins.creatorV3.reviewNoReads')}</p>
						{/each}
						<p>{$t('plugins.creatorV3.reviewNoWasm')}</p>
						<p>{$t('plugins.creatorV3.reviewCapabilities')}</p>
					{:else if !isV3(bundle.upload.manifest)}
						<p>
							{$t('plugins.creator.targetForm', {
								values: { form: bundle.upload.manifest.ui.form_key }
							})}
						</p>
						<h4 class="font-medium">{$t('plugins.requestedCapabilities')}</h4>
						<ul class="list-disc pl-5">
							{#each bundle.upload.manifest.capabilities as item (item.name)}<li>
									{$t(`plugins.capability.${item.name}`)}
								</li>{/each}
						</ul>
					{/if}
					<label class="flex items-start gap-2">
						<input
							type="checkbox"
							bind:checked={reviewed}
							disabled={!sampleRan || installing}
							class="mt-1"
						/>
						<span>{$t('plugins.creator.reviewed')}</span>
					</label>
					{#if !sampleRan}<p role="status" class="text-sm">
							{$t(review ? 'plugins.creatorV3.previewUi' : 'plugins.creator.previewFirst')}
						</p>{/if}
					<div class="flex flex-wrap items-center gap-2">
						<Button variant="secondary" disabled={!reviewed || !sampleRan} onclick={download}
							>{$t('plugins.creator.export')}</Button
						>
						<Button
							disabled={!reviewed || !sampleRan || installed}
							loading={installing}
							onclick={install}
							>{$t(personal ? 'plugins.library.save' : 'plugins.installPackage')}</Button
						>
					</div>
					{#if installed}
						<p role="status">{$t(personal ? 'plugins.library.saved' : 'plugins.installed')}</p>
						<a href={settingsUrl} class="text-fg underline"
							>{$t(personal ? 'plugins.library.title' : 'plugins.creator.reviewInstalled')}</a
						>
					{/if}
				</div>
			</section>
		{/if}
	{/if}
</div>
