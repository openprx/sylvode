<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import {
		pluginLibraryApi,
		type LibraryProject,
		type PersonalPluginPackage,
		type PluginUploadV3
	} from '#lib/api/plugin-packages.js';
	import { listPersonalDrafts } from '#lib/plugins/creator/personal-drafts.js';
	import { readPluginUpload } from '#lib/plugins/upload.js';
	import Button from '#lib/ui/button/Button.svelte';
	import PersonalPluginVersion from '#lib/components/plugins/PersonalPluginVersion.svelte';

	let packages = $state<PersonalPluginPackage[]>([]);
	let projects = $state<LibraryProject[]>([]);
	let drafts = $state<ReturnType<typeof listPersonalDrafts>>([]);
	let loading = $state(true);
	let error = $state('');
	let importing = $state(false);
	let importError = $state('');
	let importNotice = $state('');
	let upload = $state<PluginUploadV3 | null>(null);
	let importBusy = $state(false);
	let selection = 0;
	let generation = 0;
	let active = true;

	onMount(() => {
		const signal = apiClient.authSessionSignal();
		const clear = () => {
			active = false;
			generation++;
			packages = projects = drafts = [];
			upload = null;
			loading = false;
		};
		signal.addEventListener('abort', clear, { once: true });
		const actor = apiClient.getAuthPrincipal();
		if (actor) {
			drafts = listPersonalDrafts(actor);
			void refresh();
		} else clear();
		return () => {
			clear();
			signal.removeEventListener('abort', clear);
		};
	});

	async function refresh() {
		if (!active) return;
		const request = ++generation;
		error = '';
		const result = await pluginLibraryApi.list();
		if (request !== generation) return;
		loading = false;
		if (result.code === 0 && result.data) {
			packages = result.data.items;
			projects = result.data.projects;
		} else error = 'plugins.library.loadFailed';
	}

	async function selectFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		const request = ++selection;
		importNotice = importError = '';
		upload = null;
		input.value = '';
		if (!file) return;
		try {
			const candidate = await readPluginUpload(file);
			if (active && request === selection) upload = candidate;
		} catch {
			if (active && request === selection) importError = 'plugins.badPackage';
		}
	}

	async function saveImport() {
		if (!active || !upload || importBusy) return;
		importBusy = true;
		importNotice = importError = '';
		const result = await pluginLibraryApi.save(upload);
		if (!active) return;
		if (result.code === 0 && result.data) {
			upload = null;
			importNotice = 'plugins.library.saved';
			await refresh();
		} else importError = 'plugins.library.saveFailed';
		importBusy = false;
	}
</script>

<div class="mx-auto max-w-5xl space-y-6 px-1">
	<header class="flex flex-wrap items-start justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">{$t('plugins.library.title')}</h1>
			<p class="mt-2 text-sm text-fg-muted">{$t('plugins.library.summary')}</p>
		</div>
		<div class="flex flex-wrap gap-2">
			<Button
				variant="secondary"
				aria-expanded={importing}
				onclick={() => {
					importing = !importing;
				}}>{$t('plugins.library.import')}</Button
			>
			<Button
				variant="primary"
				onclick={() => goto(resolve(`/(app)/plugins/creator?draft=${crypto.randomUUID()}`))}
				>{$t('plugins.creator.launch')}</Button
			>
			<Button
				variant="secondary"
				onclick={() =>
					goto(
						resolve(
							`/(app)/plugins/creator?draft=${crypto.randomUUID()}&template=ui_view&slot=project.view`
						)
					)}>{$t('plugins.creatorV3.newUiView')}</Button
			>
		</div>
	</header>

	{#if importing}
		<section
			aria-label={$t('plugins.library.import')}
			class="space-y-3 rounded-card border border-line bg-raised p-4"
		>
			<label class="grid gap-2 text-sm">
				{$t('plugins.packageFile')}
				<input
					type="file"
					accept=".json,application/json"
					disabled={importBusy}
					onchange={selectFile}
				/>
			</label>
			{#if upload}
				<div class="space-y-2 border-l-2 border-line-strong pl-3 text-sm">
					<p class="font-medium">{upload.manifest.name} · v{upload.manifest.version}</p>
					<p>{upload.manifest.key} · {upload.manifest.publisher}</p>
					<p>
						{$t('plugins.layoutV3.installSummary', {
							values: {
								count: upload.manifest.contributions.length,
								reads: upload.manifest.reads.length
							}
						})}
					</p>
					<p>{$t(`plugins.layoutV3.profile.${upload.manifest.profile}`)}</p>
					<Button variant="primary" loading={importBusy} onclick={saveImport}
						>{$t('plugins.library.save')}</Button
					>
				</div>
			{/if}
			{#if importError}<p role="alert" class="text-sm text-danger">
					{$t(importError)}
				</p>{/if}
			{#if importNotice}<p role="status" class="text-sm">{$t(importNotice)}</p>{/if}
		</section>
	{/if}

	{#if drafts.length}
		<section aria-labelledby="personal-drafts" class="space-y-2">
			<h2 id="personal-drafts" class="font-semibold">{$t('plugins.library.drafts')}</h2>
			<ul class="divide-y divide-line rounded-card border border-line bg-raised px-4">
				{#each drafts as entry (`${entry.templateId}:${entry.id}`)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-3 text-sm">
						<span class="break-all"
							>{entry.draft.name || $t('plugins.library.untitled')} · v{entry.draft.version}</span
						>
						<a
							href={resolve(
								`/(app)/plugins/creator?draft=${encodeURIComponent(entry.id)}&template=${entry.templateId}`
							)}
							aria-label={$t('plugins.library.editNamed', {
								values: { name: entry.draft.name || $t('plugins.library.untitled') }
							})}
							class="text-fg underline">{$t('plugins.library.edit')}</a
						>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="space-y-3" aria-labelledby="saved-personal-versions">
		<h2 id="saved-personal-versions" class="font-semibold">{$t('plugins.library.versions')}</h2>
		{#if error}
			<p role="alert" class="text-sm text-danger">{$t(error)}</p>
			<Button variant="secondary" onclick={refresh}>{$t('plugins.library.retry')}</Button>
		{:else if loading}
			<p role="status" class="text-sm">{$t('common.loading')}</p>
		{:else if !packages.length}
			<p class="text-sm text-fg-muted">{$t('plugins.library.empty')}</p>
		{/if}
		{#each packages as item (item.id)}
			<PersonalPluginVersion {item} {projects} onChanged={refresh} />
		{/each}
	</section>
</div>
