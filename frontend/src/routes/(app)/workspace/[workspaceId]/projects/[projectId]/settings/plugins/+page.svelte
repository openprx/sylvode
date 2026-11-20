<script lang="ts">
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import CreatorPluginLink from '#lib/components/plugins/CreatorPluginLink.svelte';
	import { readPluginUpload } from '#lib/plugins/upload.js';
	import PluginPlacementFields from '#lib/components/plugins/PluginPlacementFields.svelte';
	import { placementDraft, validPlacementPositions } from '#lib/plugins/placements.js';
	import {
		grantOptionsV3,
		slotLabelKey,
		grantsValidV3,
		installedLayoutDraftV3,
		normalizeGrantsV3,
		toggleGrantV3,
		type GrantOptionV3
	} from '#lib/plugins/layout-v3.js';
	import ProjectPluginRequests from '#lib/components/plugins/ProjectPluginRequests.svelte';
	import PluginProjectHistory from '#lib/components/plugins/PluginProjectHistory.svelte';
	import Button from '#lib/components/Button.svelte';
	import { projectsApi } from '#lib/api/projects.js';
	import { apiClient } from '#lib/api/client.js';
	import {
		pluginCapabilities,
		isV3,
		pluginExtensionsApi,
		pluginLibraryApi,
		pluginPackagesApi,
		type InstalledPluginPackage,
		type PluginBinding,
		type PluginBindingEvent,
		type PluginCapability,
		type PluginGrant,
		type PluginPlacement,
		type PluginUpload,
		type PluginUploadV3
	} from '#lib/api/plugin-packages.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';

	const projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));
	const workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));
	let packages = $state<InstalledPluginPackage[]>([]);
	let bindings = $state<PluginBinding[]>([]);
	let history = $state<PluginBindingEvent[] | undefined>();
	let candidate = $state<PluginUpload | PluginUploadV3 | null>(null);
	let loading = $state(true);
	let allowed = $state(false);
	let busy = $state(false);
	let removing = $state<string | null>(null);
	let error = $state('');
	let backendMessage = $state('');
	let notice = $state('');
	let selected = $state<InstalledPluginPackage | null>(null);
	let grants = $state<PluginGrant[]>([]);
	let layoutSupported = $state(false);
	let extensionSupported = $state(false);
	let resetting = $state<string | null>(null);
	let placements = $state<PluginPlacement[] | undefined>();
	const validPositions = $derived(validPlacementPositions(placements));
	const selectedV3 = $derived(selected && isV3(selected.manifest) ? selected.manifest : null);
	const grantsReady = $derived(
		selectedV3 ? grantsValidV3(selectedV3, grants) : grants.includes('amount.preview')
	);
	const candidateBlocked = $derived(
		!candidate
			? false
			: isV3(candidate.manifest)
				? !extensionSupported
				: !!candidate.manifest.ui.contributions && !layoutSupported
	);
	let fileInput: HTMLInputElement | undefined = $state();
	let priorMutation: { signature: string; requestId: string } | undefined;
	let fileRead = 0;
	let scope = 0;

	$effect(() => {
		const id = projectId;
		const workspace = workspaceId;
		const mine = ++scope;
		allowed = false;
		loading = true;
		packages = [];
		bindings = [];
		history = undefined;
		layoutSupported = false;
		extensionSupported = false;
		resetting = null;
		candidate = null;
		selected = null;
		removing = null;
		error = '';
		backendMessage = '';
		notice = '';
		busy = false;
		void load(workspace, id, mine);
	});

	onMount(() => {
		const signal = apiClient.authSessionSignal();
		const onAbort = () => {
			scope++;
			allowed = false;
			candidate = null;
			selected = null;
			loading = false;
			error = 'plugins.sessionChanged';
		};
		signal.addEventListener('abort', onAbort, { once: true });
		return () => {
			scope++;
			signal.removeEventListener('abort', onAbort);
		};
	});

	async function load(workspace: string, project: string, mine: number, preserveError = false) {
		const [metadata, response] = await Promise.all([
			projectsApi.get(project),
			pluginPackagesApi.list(project)
		]);
		if (scope !== mine) return;
		loading = false;
		allowed = metadata.data?.workspace_id === workspace && response.code === 0 && !!response.data;
		if (!response.data || !allowed) {
			error = response.code === 403 ? 'plugins.manageDenied' : 'plugins.manageUnavailable';
			return;
		}
		packages = response.data.items;
		bindings = response.data.bindings;
		history = response.data.binding_history;
		layoutSupported = response.data.layout_supported === true;
		extensionSupported = response.data.extension_supported === true;
		if (!preserveError) error = '';
	}

	async function chooseFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		const mine = scope;
		const read = ++fileRead;
		candidate = null;
		notice = backendMessage = error = '';
		if (!file) return;
		try {
			const value = await readPluginUpload(file);
			if (mine === scope && read === fileRead) candidate = value;
		} catch {
			if (mine === scope && read === fileRead) error = 'plugins.badPackage';
		}
	}

	async function install() {
		if (!candidate || busy) return;
		const mine = scope;
		const id = projectId;
		busy = true;
		error = backendMessage = notice = '';
		const response = await pluginPackagesApi.install(id, candidate);
		if (mine !== scope) return;
		busy = false;
		if (response.code !== 0 || !response.data) {
			error = 'plugins.installFailed';
			backendMessage = response.message;
			return;
		}
		candidate = null;
		if (fileInput) fileInput.value = '';
		notice = 'plugins.installed';
		await load(workspaceId, id, mine);
	}

	function review(item: InstalledPluginPackage) {
		selected = item;
		const active = bindings.find((binding) => binding.key === item.manifest.key);
		grants = active?.package_id === item.id ? [...active.grants] : [];
		placements = isV3(item.manifest)
			? installedLayoutDraftV3(item, active)
			: layoutSupported
				? placementDraft(item)
				: undefined;
		if (isV3(item.manifest)) grants = normalizeGrantsV3(item.manifest, grants);
		priorMutation = undefined;
		error = backendMessage = notice = '';
	}

	function toggleGrant(grant: PluginGrant, enabled: boolean) {
		grants = selectedV3
			? toggleGrantV3(selectedV3, grants, grant, enabled)
			: enabled
				? [...grants, grant]
				: grants.filter((item) => item !== grant);
		priorMutation = undefined;
	}

	function editPlacement(
		id: string,
		change: Partial<Pick<PluginPlacement, 'enabled' | 'position'>>
	) {
		placements = placements?.map((placement) =>
			placement.id === id ? { ...placement, ...change } : placement
		);
		priorMutation = undefined;
	}

	async function updateBinding(key: string, item: InstalledPluginPackage | null) {
		if (busy || (item && (!grantsReady || !validPositions))) return;
		const id = projectId;
		const mine = scope;
		const active = bindings.find((binding) => binding.key === key);
		busy = true;
		error = backendMessage = notice = '';
		const change = {
			package_id: item?.id ?? null,
			expected_generation: active?.generation ?? 0,
			grants: item ? $state.snapshot(grants).sort() : [],
			...(item && placements ? { placements: $state.snapshot(placements) } : {})
		};
		const signature = JSON.stringify([key, change]);
		if (priorMutation?.signature !== signature)
			priorMutation = { signature, requestId: crypto.randomUUID() };
		const response = await pluginPackagesApi.setBinding(id, key, {
			...change,
			request_id: priorMutation.requestId
		});
		if (mine !== scope) return;
		busy = false;
		if (response.code !== 0) {
			error = response.code === 409 ? 'plugins.bindingChanged' : 'plugins.bindingFailed';
			backendMessage = response.message;
			if (response.code === 409) {
				selected = null;
				priorMutation = undefined;
				await load(workspaceId, id, mine, true);
			}
			return;
		}
		selected = null;
		priorMutation = undefined;
		notice = item ? 'plugins.bindingSaved' : 'plugins.disabled';
		await load(workspaceId, id, mine);
	}
	async function resetConfiguration(item: InstalledPluginPackage) {
		const key = item.manifest.key;
		const active = bindings.find((binding) => binding.key === key);
		if (busy || resetting !== key || !active || active.package_id !== null) return;
		const id = projectId;
		const mine = scope;
		busy = true;
		error = backendMessage = notice = '';
		const change = { expected_generation: active.generation };
		const signature = JSON.stringify(['reset', key, change]);
		if (priorMutation?.signature !== signature)
			priorMutation = { signature, requestId: crypto.randomUUID() };
		const response = await pluginExtensionsApi.resetConfiguration(id, key, {
			...change,
			request_id: priorMutation.requestId
		});
		if (mine !== scope) return;
		busy = false;
		if (response.code !== 0) {
			error = response.code === 409 ? 'plugins.bindingChanged' : 'plugins.layoutV3.resetFailed';
			backendMessage = response.message;
			if (response.code === 409) {
				priorMutation = undefined;
				resetting = null;
				await load(workspaceId, id, mine, true);
			}
			return;
		}
		priorMutation = undefined;
		resetting = null;
		notice = 'plugins.layoutV3.resetDone';
		await load(workspaceId, id, mine);
	}

	async function removeFromProject(item: InstalledPluginPackage) {
		if (!item.library_source_id || removing !== item.id || busy) return;
		const mine = scope;
		const id = projectId;
		const workspace = workspaceId;
		busy = true;
		error = backendMessage = notice = '';
		const response = await pluginLibraryApi.revoke(id, item.library_source_id);
		if (mine !== scope) return;
		if (response.code === 0) {
			selected = removing = null;
			notice = 'plugins.library.removedNotice';
			await load(workspace, id, mine);
		} else error = 'plugins.library.removeFailed';
		busy = false;
	}
</script>

{#snippet grantText(option: GrantOptionV3)}
	{#if option.kind === 'display'}
		{$t('plugins.layoutV3.grant.display')}
	{:else if option.kind === 'read' && option.read}
		{$t('plugins.layoutV3.grant.read', {
			values: { fields: option.read.fields.join(', '), form: option.read.form_key }
		})}
	{:else}
		{$t(`plugins.capability.${option.grant}`)}
	{/if}
{/snippet}

<div class="mx-auto max-w-5xl space-y-5 px-1">
	<header class="space-y-2">
		<a
			class="text-sm text-blue-700 underline dark:text-blue-300"
			href={`/workspace/${workspaceId}/projects/${projectId}/forms`}>{$t('plugins.backToForms')}</a
		>
		<h1 class="text-2xl font-semibold">{$t('plugins.projectSettings')}</h1>
		<a href="/plugins" class="inline-block text-sm text-blue-700 underline dark:text-blue-300">
			{$t('plugins.library.title')}
		</a>
	</header>

	{#if error}
		<p
			role="alert"
			class="rounded border border-red-300 px-3 py-2 text-sm text-danger dark:border-red-700"
		>
			{$t(error)}{#if backendMessage}
				{backendMessage}{/if}
		</p>
	{/if}
	{#if notice}<p role="status" class="text-sm">{$t(notice)}</p>{/if}
	{#if loading}<p role="status" class="text-sm">{$t('common.loading')}</p>{/if}
	{#if allowed}
		{#key projectId}
			<ProjectPluginRequests {projectId} onApproved={() => load(workspaceId, projectId, scope)} />
		{/key}
		<section
			class="space-y-3 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
		>
			<div class="flex flex-wrap items-center justify-between gap-2">
				<h2 class="font-semibold">{$t('plugins.installPackage')}</h2>
				<div class="flex flex-wrap items-center gap-2">
					{#if layoutSupported}<CreatorPluginLink
							{workspaceId}
							{projectId}
							formKey="order_line"
							slot="form.record.detail"
						/>{/if}
					{#if extensionSupported}<CreatorPluginLink
							{workspaceId}
							{projectId}
							formKey={null}
							slot="project.view"
						/>{/if}
				</div>
			</div>
			<label for="plugin-package-file" class="block text-sm">{$t('plugins.packageFile')}</label>
			<input
				id="plugin-package-file"
				type="file"
				bind:this={fileInput}
				accept=".json,application/json"
				disabled={busy}
				onchange={chooseFile}
				class="block max-w-full text-sm"
			/>
			{#if candidate}
				<div class="space-y-2 border-l-2 border-blue-500 pl-3 text-sm">
					<h3 class="font-medium">{candidate.manifest.name} · v{candidate.manifest.version}</h3>
					<p>{$t('plugins.publisher', { values: { publisher: candidate.manifest.publisher } })}</p>
					{#if isV3(candidate.manifest)}
						<p>{$t(`plugins.layoutV3.profile.${candidate.manifest.profile}`)}</p>
						<p>{$t('plugins.contributedLocations')}</p>
						<ul class="list-disc pl-5">
							{#each candidate.manifest.contributions as contribution (contribution.id)}
								<li>
									{contribution.label} · {$t(slotLabelKey(contribution.slot))} ·
									{contribution.route
										? $t('plugins.layoutV3.route', { values: { route: contribution.route } })
										: $t('plugins.layoutV3.form', { values: { form: contribution.form_key } })}
								</li>
							{/each}
						</ul>
						<p>{$t('plugins.layoutV3.grantsTitle')}</p>
						<ul class="list-disc pl-5">
							{#each grantOptionsV3(candidate.manifest) as option (option.grant)}
								<li>{@render grantText(option)}</li>
							{/each}
						</ul>
						{#if !extensionSupported}<p role="status">
								{$t('plugins.layoutV3.extensionsUnavailable')}
							</p>{/if}
					{:else}
						<p>
							{$t('plugins.compatibility', {
								values: {
									bridge: candidate.manifest.ui.bridge_version,
									form: candidate.manifest.ui.form_key
								}
							})}
						</p>
						<p>{$t('plugins.requestedCapabilities')}</p>
						<ul class="list-disc pl-5">
							{#each candidate.manifest.capabilities as capability}
								<li>
									{pluginCapabilities.includes(capability.name)
										? $t(`plugins.capability.${capability.name}`)
										: capability.name}
								</li>
							{/each}
						</ul>
						{#if candidate.manifest.ui.contributions?.length}
							<p>{$t('plugins.contributedLocations')}</p>
							<ul class="list-disc pl-5">
								{#each candidate.manifest.ui.contributions as contribution}
									<li>{contribution.label} · {$t(`plugins.slot.${contribution.slot}`)}</li>
								{/each}
							</ul>
							{#if !layoutSupported}<p role="status">{$t('plugins.layoutUnavailable')}</p>{/if}
						{/if}
					{/if}
					<Button onclick={install} loading={busy} disabled={candidateBlocked}>
						{$t('plugins.installPackage')}
					</Button>
				</div>
			{/if}
		</section>

		<section class="space-y-3" aria-labelledby="installed-plugins">
			<h2 id="installed-plugins" class="font-semibold">{$t('plugins.installedVersions')}</h2>
			{#if !packages.length}<p class="text-sm">{$t('plugins.noPackages')}</p>{/if}
			<ul class="space-y-2">
				{#each packages as item (item.id)}
					{@const binding = bindings.find((entry) => entry.key === item.manifest.key)}
					{@const active = binding?.package_id === item.id}
					<li
						class="space-y-3 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
					>
						<div class="flex flex-wrap items-start justify-between gap-2">
							<div class="min-w-0 space-y-1">
								<h3 class="font-semibold">{item.manifest.name} · v{item.manifest.version}</h3>
								<p class="text-sm">
									{$t('plugins.publisher', { values: { publisher: item.manifest.publisher } })}
								</p>
								{#if item.installed_at}
									<p class="text-xs text-fg-muted">
										{$t('plugins.installedAt', {
											values: {
												date: new Intl.DateTimeFormat($locale || 'en', {
													dateStyle: 'medium'
												}).format(new Date(item.installed_at))
											}
										})}
									</p>
								{/if}
								<p class="text-sm">
									{active
										? $t('plugins.activeVersion', { values: { generation: binding.generation } })
										: $t('plugins.inactiveVersion')}
								</p>
								{#if isV3(item.manifest)}
									<p class="text-sm">{$t(`plugins.layoutV3.profile.${item.manifest.profile}`)}</p>
									<ul class="list-disc pl-5 text-sm">
										{#each item.manifest.contributions as contribution (contribution.id)}
											<li>
												{contribution.label} · {$t(slotLabelKey(contribution.slot))} ·
												{contribution.route
													? $t('plugins.layoutV3.route', { values: { route: contribution.route } })
													: $t('plugins.layoutV3.form', {
															values: { form: contribution.form_key }
														})}
											</li>
										{/each}
									</ul>
								{/if}
							</div>
							<Button variant="secondary" onclick={() => review(item)} disabled={busy}
								>{$t('plugins.reviewAccess')}</Button
							>
						</div>
						{#if item.library_source_id}
							{#if removing === item.id}
								<div class="flex flex-wrap items-center gap-2 text-sm">
									<p>{$t('plugins.library.removeQuestion')}</p>
									<Button
										variant="danger"
										size="sm"
										disabled={busy}
										onclick={() => removeFromProject(item)}>{$t('plugins.library.confirm')}</Button
									>
									<Button
										variant="secondary"
										size="sm"
										disabled={busy}
										onclick={() => {
											removing = null;
										}}>{$t('common.cancel')}</Button
									>
								</div>
							{:else}
								<Button
									variant="secondary"
									size="sm"
									disabled={busy}
									onclick={() => {
										removing = item.id;
									}}
								>
									{$t('plugins.library.remove')}
								</Button>
							{/if}
						{/if}
						{#if isV3(item.manifest) && binding && binding.package_id === null && packages.find((entry) => entry.manifest.key === item.manifest.key)?.id === item.id}
							{#if resetting === item.manifest.key}
								<div class="flex flex-wrap items-center gap-2 text-sm">
									<p>
										{$t('plugins.layoutV3.resetConfirm', { values: { key: item.manifest.key } })}
									</p>
									<Button
										variant="danger"
										size="sm"
										loading={busy}
										onclick={() => resetConfiguration(item)}>{$t('plugins.library.confirm')}</Button
									>
									<Button
										variant="secondary"
										size="sm"
										disabled={busy}
										onclick={() => {
											resetting = null;
										}}>{$t('common.cancel')}</Button
									>
								</div>
							{:else}
								<div class="flex flex-wrap items-center gap-2 text-sm">
									<p class="text-fg-muted">
										{$t('plugins.layoutV3.resetHelp')}
									</p>
									<Button
										variant="secondary"
										size="sm"
										disabled={busy}
										onclick={() => {
											resetting = item.manifest.key;
										}}>{$t('plugins.layoutV3.reset')}</Button
									>
								</div>
							{/if}
						{/if}
						{#if selected?.id === item.id}
							<fieldset class="space-y-2 rounded border border-slate-300 p-3 dark:border-slate-700">
								<legend class="px-1 text-sm font-medium">{$t('plugins.grantAccess')}</legend>
								{#if isV3(item.manifest)}
									<p class="text-sm text-fg-muted">
										{$t('plugins.layoutV3.grantsHelp')}
									</p>
									{#each grantOptionsV3(item.manifest) as option (option.grant)}
										<label class="flex items-start gap-2 text-sm">
											<input
												type="checkbox"
												class="mt-1"
												checked={grants.includes(option.grant)}
												disabled={busy}
												onchange={(event) => toggleGrant(option.grant, event.currentTarget.checked)}
											/>
											<span>{@render grantText(option)}</span>
										</label>
									{/each}
									{#if !grants.includes('ui.display')}<p class="text-sm">
											{$t('plugins.layoutV3.displayRequired')}
										</p>{/if}
								{:else}
									{#each item.manifest.capabilities as capability (capability.name)}
										<label class="flex items-start gap-2 text-sm">
											<input
												type="checkbox"
												class="mt-1"
												checked={grants.includes(capability.name)}
												disabled={busy}
												onchange={(event) =>
													toggleGrant(capability.name, event.currentTarget.checked)}
											/>
											<span>{$t(`plugins.capability.${capability.name}`)}</span>
										</label>
									{/each}
									{#if !grants.includes('amount.preview')}<p class="text-sm">
											{$t('plugins.previewGrantRequired')}
										</p>{/if}
								{/if}
							</fieldset>
							{#if placements && isV3(item.manifest)}
								<PluginPlacementFields
									contributions={item.manifest.contributions}
									{placements}
									disabled={busy}
									onChange={editPlacement}
								/>
							{:else if placements && !isV3(item.manifest) && item.manifest.ui.contributions}
								<PluginPlacementFields
									contributions={item.manifest.ui.contributions}
									{placements}
									disabled={busy}
									onChange={editPlacement}
								/>
							{/if}
							<div class="flex flex-wrap items-center gap-2">
								<Button
									disabled={!grantsReady || !validPositions}
									loading={busy}
									onclick={() => updateBinding(item.manifest.key, item)}
								>
									{$t(
										active
											? placements
												? 'plugins.saveGrantsAndLayout'
												: 'plugins.saveGrants'
											: binding?.package_id
												? 'plugins.switchVersion'
												: 'plugins.enableVersion'
									)}
								</Button>
								{#if active}
									<Button
										variant="danger"
										disabled={busy}
										onclick={() => updateBinding(item.manifest.key, null)}
										>{$t('plugins.disablePlugin')}</Button
									>
								{/if}
								<Button
									variant="secondary"
									disabled={busy}
									onclick={() => {
										selected = null;
										priorMutation = undefined;
									}}>{$t('common.cancel')}</Button
								>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		</section>
		{#key projectId}<PluginProjectHistory {projectId} {packages} {history} />{/key}
	{/if}
</div>
