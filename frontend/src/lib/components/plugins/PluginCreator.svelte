<script lang="ts">
	import { onMount, tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { personalDraftKey } from '#lib/plugins/creator/personal-drafts.js';
	import Button from '#lib/components/Button.svelte';
	import CreatorPanelPreview from '#lib/components/plugins/CreatorPanelPreview.svelte';
	import { apiClient } from '#lib/api/client.js';
	import { projectsApi } from '#lib/api/projects.js';
	import {
		pluginLibraryApi,
		pluginPackagesApi,
		type PluginSlot
	} from '#lib/api/plugin-packages.js';
	import {
		actionSlot,
		buildCreatorPackage,
		creatorSlot,
		detailSlot,
		invalidDraftFields,
		readDraft,
		type CreatorDraft,
		type CreatorPackage
	} from '#lib/plugins/creator/template.js';

	type CreatorScope =
		| { kind: 'personal'; draftId: string }
		| { kind: 'project'; workspaceId: string; projectId: string };

	let {
		context,
		initialSlot,
		formKey
	}: {
		context: CreatorScope;
		initialSlot: string | null;
		formKey: string;
	} = $props();

	const personal = $derived(context.kind === 'personal');
	const origin = $derived(creatorSlot(initialSlot));
	const settingsUrl = $derived(
		context.kind === 'project'
			? `/workspace/${context.workspaceId}/projects/${context.projectId}/settings/plugins`
			: '/plugins'
	);
	let draft = $state<CreatorDraft>(readDraft(null));
	let allowed = $state(false);
	let checking = $state(true);
	let building = $state(false);
	let installing = $state(false);
	let error = $state('');
	let backendMessage = $state('');
	let installed = $state(false);
	let bundle = $state.raw<CreatorPackage | null>(null);
	let previewSlot = $state<PluginSlot>(detailSlot);
	let sampleRan = $state(false);
	let reviewed = $state(false);
	let reviewStart: HTMLHeadingElement | undefined = $state();
	let storageKey = '';
	let scope = 0;
	let revision = $state(0);

	const invalid = $derived(invalidDraftFields(draft));
	const details = ['detailLabel', 'detailTitle', 'detailText'] as const;
	const actions = ['actionLabel', 'actionTitle', 'actionText'] as const;
	const metadata = ['key', 'name', 'publisher', 'version', 'formKey'] as const;

	$effect(() => {
		const mine = ++scope;
		const current = context;
		const key = formKey;
		const start = origin;
		const actor = apiClient.getAuthPrincipal();
		allowed = false;
		checking = true;
		resetBuild();
		previewSlot = start;
		storageKey = '';
		if (!actor) {
			checking = false;
			error = 'plugins.manageDenied';
		} else if (current.kind === 'personal') {
			untrack(() => setup(personalDraftKey(actor, current.draftId), key, true));
		} else if (current.workspaceId && current.projectId) {
			void authorizeProject(current, mine, actor, key);
		} else {
			checking = false;
			error = 'plugins.manageDenied';
		}
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
		mine: number,
		actor: string,
		formKey: string
	) {
		const [metadata, catalog] = await Promise.all([
			projectsApi.get(project.projectId),
			pluginPackagesApi.list(project.projectId)
		]);
		if (scope !== mine) return;
		if (
			metadata.data?.workspace_id !== project.workspaceId ||
			catalog.code !== 0 ||
			catalog.data?.layout_supported !== true
		) {
			checking = false;
			error = catalog.code === 403 ? 'plugins.manageDenied' : 'plugins.creator.unavailable';
			return;
		}
		setup(`sv.plugin.creator.v1:${actor}:${project.projectId}:${formKey}`, formKey, false);
	}

	function setup(key: string, formKey: string, persistNew: boolean) {
		allowed = true;
		checking = false;
		storageKey = key;
		let prior: string | null = null;
		try {
			if (key) prior = localStorage.getItem(key);
		} catch {
			/* The draft remains usable in memory. */
		}
		draft = readDraft(prior, formKey);
		if (!prior && persistNew) storeDraft();
	}

	function storeDraft() {
		try {
			if (storageKey) localStorage.setItem(storageKey, JSON.stringify(draft));
		} catch {
			/* Browser storage can be disabled. */
		}
	}

	function resetBuild() {
		bundle = null;
		reviewed = sampleRan = building = installing = installed = false;
		error = backendMessage = '';
	}

	function edit(field: keyof CreatorDraft, value: string) {
		draft = { ...draft, [field]: value };
		revision++;
		resetBuild();
		storeDraft();
	}

	async function build() {
		if (!allowed || invalid.length || building || installing) return;
		const buildRevision = ++revision;
		const mine = scope;
		resetBuild();
		building = true;
		try {
			const output = await buildCreatorPackage($state.snapshot(draft));
			if (buildRevision !== revision || mine !== scope) return;
			bundle = output;
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
			class="rounded-md border border-slate-300 bg-transparent px-3 py-2 dark:border-slate-600"
			value={draft[field]}
			maxlength={field.endsWith('Text') ? 320 : 120}
			required
			disabled={building || installing}
			aria-invalid={invalid.includes(field)}
			aria-describedby="creator-field-help"
			oninput={(event) => edit(field, event.currentTarget.value)}
		/>
	</label>
{/snippet}

<div class="mx-auto max-w-5xl space-y-5 px-1">
	<header class="space-y-2">
		<a href={settingsUrl} class="text-sm text-blue-700 underline dark:text-blue-300"
			>{$t(personal ? 'plugins.library.title' : 'plugins.projectSettings')}</a
		>
		<h1 class="text-2xl font-semibold">{$t('plugins.creator.title')}</h1>
		<p class="text-sm text-slate-600 dark:text-slate-300">{$t('plugins.creator.template')}</p>
	</header>
	{#if error}
		<p
			role="alert"
			class="rounded border border-red-300 px-3 py-2 text-sm text-red-800 dark:border-red-700 dark:text-red-200"
		>
			{$t(error)}
			{backendMessage}
		</p>
	{/if}
	{#if checking}<p role="status" class="text-sm">{$t('common.loading')}</p>{/if}
	{#if allowed}
		<section
			class="space-y-4 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
		>
			<h2 class="font-semibold">{$t('plugins.creator.draft')}</h2>
			<p class="text-sm text-slate-600 dark:text-slate-300">
				{$t('plugins.creator.origin', { values: { slot: $t(`plugins.slot.${origin}`) } })}
				{#if personal}
					{$t('plugins.library.localDraft')}
				{:else}
					{$t('plugins.creator.origin', { values: { slot: $t(`plugins.slot.${origin}`) } })}
					{$t('plugins.creator.localDraft')}
				{/if}
			</p>
			<form
				class="space-y-4"
				onsubmit={(event) => {
					event.preventDefault();
					void build();
				}}
			>
				<fieldset class="grid gap-3 sm:grid-cols-2">
					<legend class="pb-2 text-sm font-semibold">{$t('plugins.creator.package')}</legend>
					{#each metadata as field (field)}{@render fieldEditor(field)}{/each}
				</fieldset>
				<p id="creator-field-help" class="text-xs text-slate-600 dark:text-slate-300">
					{$t('plugins.creator.fieldHelp')}
				</p>
				<div class="grid gap-3 sm:grid-cols-2">
					{#each origin === actionSlot ? [actionSlot, detailSlot] : [detailSlot, actionSlot] as slot (slot)}
						<fieldset class="space-y-2 rounded border border-slate-300 p-3 dark:border-slate-700">
							<legend class="px-1 text-sm font-medium">{$t(`plugins.slot.${slot}`)}</legend>
							{#each slot === actionSlot ? actions : details as field (field)}{@render fieldEditor(
									field
								)}{/each}
						</fieldset>
					{/each}
				</div>
				{#if invalid.length}<p role="status" class="text-sm">
						{$t('plugins.creator.invalidDraft')}
					</p>{/if}
				<Button type="submit" loading={building} disabled={invalid.length > 0 || installing}
					>{$t('plugins.creator.build')}</Button
				>
			</form>
		</section>
		{#if bundle}
			<section
				class="space-y-3 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
			>
				<h2 class="font-semibold" tabindex="-1" bind:this={reviewStart}>
					{$t('plugins.creator.review')}
				</h2>
				<p class="text-sm">{$t('plugins.creator.previewRecord')}</p>
				<label class="flex flex-wrap items-center gap-2 text-sm">
					{$t('plugins.creator.previewLocation')}
					<select
						bind:value={previewSlot}
						class="rounded border border-slate-300 bg-transparent px-3 py-1 dark:border-slate-700"
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
				<div class="space-y-2 border-t border-slate-300 pt-3 text-sm dark:border-slate-700">
					<h3 class="font-medium">
						{bundle.upload.manifest.name} · v{bundle.upload.manifest.version}
					</h3>
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
							{$t('plugins.creator.previewFirst')}
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
						<a href={settingsUrl} class="text-blue-700 underline dark:text-blue-300"
							>{$t(personal ? 'plugins.library.title' : 'plugins.creator.reviewInstalled')}</a
						>
					{/if}
				</div>
			</section>
		{/if}
	{/if}
</div>
