<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import DraftsSection from '#lib/project/drafts/DraftsSection.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Input from '#lib/components/Input.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import {
		allRequests,
		confirmedFor,
		grantsFor,
		marketApi,
		marketDetailApi,
		catalogueApi,
		type BindChoice,
		type CallRow,
		type InstallPreview,
		type Installation,
		type InvocationRow,
		type PackageDetail,
		type PackageSummary
	} from './api';
	import { indexApi } from './index-api';
	import { updateAvailable } from './index-logic';
	import { dataAccess, groupRequests, opCounts, publicPages, riskTone } from './logic';

	let {
		workspaceId,
		embedded = false,
		installDigest = null
	}: {
		workspaceId: string;
		/** Inside the Market page: the page title is the market's. */
		embedded?: boolean;
		/** A package chosen on the Discover tab: the install dialog opens with it. */
		installDigest?: string | null;
	} = $props();

	/** The installation routes are for owners and administrators; a member's list is refused. */
	let admin = $state(false);

	let loading = $state(true);
	let installations = $state<Installation[]>([]);
	let packages = $state<PackageSummary[]>([]);

	// Request dialog.
	let requestOpen = $state(false);
	let digest = $state('');
	let detail = $state<PackageDetail | null>(null);
	let scopeKind = $state<'workspace' | 'project'>('workspace');
	let projectId = $state('');
	let bindKind = $state<Record<string, 'create' | 'existing'>>({});
	let bindForm = $state<Record<string, string>>({});
	let checked = $state<InstallPreview | null>(null);
	let busy = $state(false);

	// Review dialog.
	let reviewOpen = $state(false);
	let review = $state<InstallPreview | null>(null);
	let reviewing = $state<Installation | null>(null);
	let ticked = $state<boolean[]>([]);
	let reviewDetail = $state<PackageDetail | null>(null);

	// Invocation log dialog.
	let logOpen = $state(false);
	let logFor = $state<Installation | null>(null);
	let log = $state<InvocationRow[]>([]);
	let calls = $state<CallRow[]>([]);

	/** Operations behind each plugin-exposed capability id; `null` until the catalogue answered. */
	let counts = $state.raw<Map<string, number> | null>(null);

	onMount(async () => {
		await load();
		if (installDigest && admin) {
			openRequest();
			digest = installDigest;
			await choosePackage();
		}
	});

	async function load() {
		loading = true;
		const [list, pkgs] = await Promise.all([
			marketApi.list(workspaceId),
			marketApi.packages(workspaceId)
		]);
		admin = list.code === 0;
		if (list.code !== 0 && list.code !== 403) {
			toast.error(describeResult(list, get(t)('market.loadFailed')));
		}
		installations = list.data?.items ?? [];
		packages = pkgs.data?.items ?? [];
		loading = false;
		void loadUpdates();
	}

	/** The newest compatible version the index lists, per `source/plugin id` of an imported package. */
	let latestByOrigin = $state<Record<string, string | null>>({});

	function originKey(origin: { source: string; plugin_id: string }): string {
		return `${origin.source}/${origin.plugin_id}`;
	}

	/** Where the package of an installation came from, when it was imported from a public index. */
	function originOf(installation: Installation) {
		const generation =
			installation.generations.find((item) => item.generation === installation.active_generation) ??
			pendingOf(installation);
		return packages.find((pkg) => pkg.digest === generation?.package.digest)?.origin ?? null;
	}

	/** Asks the cached index once per imported plugin; a source that is hidden or down just adds nothing. */
	async function loadUpdates() {
		const wanted: Record<string, { source: string; plugin_id: string }> = {};
		for (const installation of installations) {
			if (installation.status === 'revoked') continue;
			const origin = originOf(installation);
			if (origin) wanted[originKey(origin)] = origin;
		}
		const found: Record<string, string | null> = {};
		await Promise.all(
			Object.entries(wanted).map(async ([key, origin]) => {
				const result = await indexApi.plugin(workspaceId, origin.source, origin.plugin_id);
				found[key] = result.code === 0 ? (result.data?.latest_compatible ?? null) : null;
			})
		);
		latestByOrigin = found;
	}

	function updateOf(installation: Installation): string | null {
		const origin = originOf(installation);
		const generation = installation.generations.find(
			(item) => item.generation === installation.active_generation
		);
		if (!origin || !generation) return null;
		const latest = latestByOrigin[originKey(origin)] ?? null;
		return updateAvailable(generation.package.version, latest) ? latest : null;
	}

	function scopeBody() {
		return scopeKind === 'project'
			? { kind: 'project' as const, project_id: projectId.trim() }
			: { kind: 'workspace' as const };
	}

	function bindMap(): Record<string, BindChoice> {
		const out: Record<string, BindChoice> = {};
		for (const form of detail?.analysis.forms ?? []) {
			out[form.bind] =
				bindKind[form.bind] === 'existing'
					? { form_id: (bindForm[form.bind] ?? '').trim() }
					: { create: true };
		}
		return out;
	}

	function openRequest() {
		digest = '';
		detail = null;
		checked = null;
		scopeKind = 'workspace';
		projectId = '';
		bindKind = {};
		bindForm = {};
		requestOpen = true;
	}

	async function choosePackage() {
		checked = null;
		detail = null;
		if (!digest) return;
		const response = await marketDetailApi.package(workspaceId, digest);
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response, get(t)('market.previewFailed')));
			return;
		}
		detail = response.data;
		bindKind = Object.fromEntries(
			response.data.analysis.forms.map((form) => [form.bind, form.create ? 'create' : 'existing'])
		);
	}

	async function check() {
		busy = true;
		const response = await marketApi.preview(workspaceId, {
			package_digest: digest,
			scope: scopeBody(),
			bind_map: bindMap()
		});
		busy = false;
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response, get(t)('market.previewFailed')));
			return;
		}
		checked = response.data;
		void ensureCatalogue();
	}

	async function submitRequest() {
		busy = true;
		const response = await marketApi.request(workspaceId, {
			package_digest: digest,
			scope: scopeBody(),
			bind_map: bindMap()
		});
		busy = false;
		if (response.code !== 0) {
			toast.error(describeResult(response, get(t)('market.requestFailed')));
			return;
		}
		toast.success(get(t)('market.requested'));
		requestOpen = false;
		await load();
	}

	/** The operation counts of a review; a catalogue that does not load only drops the counts. */
	async function ensureCatalogue() {
		if (counts) return;
		const response = await catalogueApi.plugin();
		if (response.code === 0 && response.data) counts = opCounts(response.data.capabilities);
	}

	async function openReview(installation: Installation) {
		reviewing = installation;
		review = null;
		reviewDetail = null;
		reviewOpen = true;
		void ensureCatalogue();
		const response = await marketApi.review(installation.id);
		if (response.code !== 0 || !response.data) {
			toast.error(describeResult(response, get(t)('market.actionFailed')));
			reviewOpen = false;
			return;
		}
		review = response.data;
		ticked = allRequests(response.data.requests);
		const found = await marketDetailApi.package(workspaceId, response.data.package.digest);
		reviewDetail = found.code === 0 ? found.data : null;
	}

	/** Ticking a destructive id asks first: it is granted only with the explicit confirm. */
	async function toggle(index: number, on: boolean) {
		const request = review?.requests[index];
		if (on && request?.risk === 'destructive') {
			const confirmed = await confirmAction({
				tone: 'destructive',
				title: get(t)('market.destructiveTitle'),
				body: get(t)('market.destructiveBody', { values: { name: words(request.capability) } }),
				confirmLabel: get(t)('market.destructiveGrant')
			});
			if (!confirmed) {
				ticked = [...ticked];
				return;
			}
		}
		ticked = ticked.map((value, at) => (at === index ? on : value));
	}

	async function approve() {
		if (!reviewing || !review) return;
		busy = true;
		const response = await marketApi.approve(
			reviewing.id,
			review.generation,
			grantsFor(review.requests, ticked),
			confirmedFor(review.requests, ticked)
		);
		busy = false;
		if (response.code !== 0) {
			toast.error(describeResult(response, get(t)('market.actionFailed')));
			return;
		}
		toast.success(get(t)('market.approved'));
		reviewOpen = false;
		await load();
	}

	async function reject() {
		if (!reviewing || !review) return;
		busy = true;
		const response = await marketApi.reject(reviewing.id, review.generation);
		busy = false;
		if (response.code !== 0) {
			toast.error(describeResult(response, get(t)('market.actionFailed')));
			return;
		}
		toast.success(get(t)('market.rejected'));
		reviewOpen = false;
		await load();
	}

	/** Disable and revoke name the forms whose writes would stop before they ask. */
	async function stop(installation: Installation, kind: 'disable' | 'revoke') {
		const detailed = await marketDetailApi.get(installation.id);
		const forms = detailed.data?.forms_using_rules ?? [];
		const values = { values: { name: installation.package_key } };
		const list = forms.length
			? `\n\n${get(t)('market.formsUsingRules')}\n${forms.map((form) => `- ${form.name}`).join('\n')}`
			: '';
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: get(t)(kind === 'disable' ? 'market.disableTitle' : 'market.revokeTitle', values),
			body: get(t)(kind === 'disable' ? 'market.disableBody' : 'market.revokeBody') + list,
			confirmLabel: get(t)(kind === 'disable' ? 'market.disable' : 'market.revoke')
		});
		if (!confirmed) return;
		const response =
			kind === 'disable'
				? await marketApi.disable(installation.id)
				: await marketApi.revoke(installation.id);
		if (response.code !== 0) {
			toast.error(describeResult(response, get(t)('market.actionFailed')));
			return;
		}
		toast.success(get(t)(kind === 'disable' ? 'market.disabled' : 'market.revoked'));
		await load();
	}

	async function enable(installation: Installation) {
		const response = await marketApi.enable(installation.id);
		if (response.code !== 0) {
			toast.error(describeResult(response, get(t)('market.actionFailed')));
			return;
		}
		toast.success(get(t)('market.enabled'));
		await load();
	}

	async function openLog(installation: Installation) {
		logFor = installation;
		log = [];
		calls = [];
		logOpen = true;
		const [response, written] = await Promise.all([
			marketApi.invocations(installation.id),
			marketApi.calls(installation.id)
		]);
		log = response.data?.items ?? [];
		calls = written.data?.items ?? [];
	}

	function tone(status: Installation['status']): 'neutral' | 'success' | 'warning' | 'danger' {
		if (status === 'active') return 'success';
		if (status === 'pending') return 'warning';
		if (status === 'revoked') return 'danger';
		return 'neutral';
	}

	/** A capability in words; an id the catalogue of the UI does not know is shown as it is. */
	function words(capability: string): string {
		const key = `market.cap.${capability}`;
		const text = get(t)(key);
		return text === key ? capability : text;
	}

	function tierLabel(tier: string): string {
		const key = `market.tier.${tier}`;
		const text = get(t)(key);
		return text === key ? tier : text;
	}

	function areaName(area: string): string {
		const key = `market.areas.${area}.name`;
		const text = get(t)(key);
		return text === key ? area : text;
	}

	function areaDescription(area: string): string {
		const key = `market.areas.${area}.desc`;
		const text = get(t)(key);
		return text === key ? '' : text;
	}

	function createdList(values: string[]): string {
		return values.join(', ');
	}

	const sortedPackages = $derived(
		[...packages].sort((a, b) => a.package_key.localeCompare(b.package_key))
	);
	/** The console page of the site the package's pages live on (project scope, active, with pages). */
	function siteHref(installation: Installation): string | null {
		const project = installation.scope.project_id;
		if (installation.status !== 'active' || !installation.site_id || !project) return null;
		return `/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(project)}/sites/${encodeURIComponent(installation.site_id)}`;
	}

	const pendingOf = (installation: Installation) =>
		installation.generations.find((generation) => generation.status === 'pending');
</script>

<div class="space-y-6 p-6">
	<div class="flex items-start justify-between gap-4">
		<div>
			{#if !embedded}
				<h1 class="text-2xl font-semibold text-fg">{$t('market.title')}</h1>
			{/if}
			<p class="mt-1 max-w-2xl text-sm text-fg-muted">{$t('market.lead')}</p>
		</div>
		{#if admin}
			<Button variant="primary" onclick={openRequest}>{$t('market.request')}</Button>
		{/if}
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('market.loading')}
		</div>
	{:else if !admin}
		<div
			class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted"
			data-market-admin-only
		>
			{$t('market.adminOnly')}
		</div>
	{:else if installations.length === 0}
		<div class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted">
			{$t('market.empty')}
		</div>
	{:else}
		<div class="overflow-x-auto rounded-card border border-line bg-raised">
			<table class="min-w-full divide-y divide-line">
				<thead class="bg-nav">
					<tr>
						{#each ['colPackage', 'colVersion', 'colGeneration', 'colScope', 'colStatus'] as column (column)}
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t(`market.${column}`)}</th
							>
						{/each}
						<th class="px-4 py-3"></th>
					</tr>
				</thead>
				<tbody class="divide-y divide-line">
					{#each installations as installation (installation.id)}
						{@const active = installation.generations.find(
							(generation) => generation.generation === installation.active_generation
						)}
						{@const origin = originOf(installation)}
						{@const update = updateOf(installation)}
						<tr>
							<td class="px-4 py-3 text-sm text-fg">
								{installation.package_key}
								{#if origin}
									<div class="mt-1 flex flex-wrap items-center gap-2" data-market-origin>
										<StatusChip
											>{$t('market.index.originBadge', {
												values: { source: origin.source }
											})}</StatusChip
										>
										{#if update}
											<StatusChip tone="warning" data-market-update-available
												>{$t('market.index.updateAvailable', {
													values: { version: update }
												})}</StatusChip
											>
										{/if}
									</div>
								{/if}
							</td>
							<td class="px-4 py-3 text-sm text-fg-muted">
								{active?.package.version ?? pendingOf(installation)?.package.version ?? ''}
								{#if pendingOf(installation) && active}
									<span class="ml-1 text-warning">→ {pendingOf(installation)?.package.version}</span
									>
								{/if}
							</td>
							<td class="px-4 py-3 text-sm text-fg-muted">
								{installation.active_generation ?? '-'}
								{#if pendingOf(installation)}
									<span class="ml-1 text-warning">→ {pendingOf(installation)?.generation}</span>
								{/if}
							</td>
							<td class="px-4 py-3 text-sm text-fg-muted">
								{installation.scope.kind === 'project'
									? $t('market.scopeProjectShort')
									: $t('market.scopeWorkspaceShort')}
							</td>
							<td class="px-4 py-3 text-sm">
								<StatusChip tone={tone(installation.status)}
									>{$t(`market.status.${installation.status}`)}</StatusChip
								>
								{#if installation.status === 'active' && installation.approver_active === false}
									<div class="mt-1 text-xs text-warning">{$t('market.needsReapproval')}</div>
								{/if}
								{#if installation.disabled_reason}
									<div class="mt-1 text-xs text-fg-muted">
										{$t(`market.disabledReason.${installation.disabled_reason}`)}
									</div>
								{/if}
							</td>
							<td class="px-4 py-3 text-right">
								<div class="flex flex-wrap items-center justify-end gap-2">
									{#if siteHref(installation)}
										<Button
											variant="text"
											size="sm"
											data-market-open-site
											onclick={() => {
												// eslint-disable-next-line svelte/no-navigation-without-resolve -- console path built in full
												void goto(siteHref(installation) ?? '');
											}}>{$t('market.openSite')}</Button
										>
									{/if}
									{#if installation.status !== 'revoked'}
										<Button variant="text" size="sm" onclick={() => openReview(installation)}
											>{$t('market.review')}</Button
										>
									{/if}
									{#if installation.status === 'active'}
										<Button variant="text" size="sm" onclick={() => stop(installation, 'disable')}
											>{$t('market.disable')}</Button
										>
									{:else if installation.status === 'disabled'}
										<Button variant="text" size="sm" onclick={() => enable(installation)}
											>{$t('market.enable')}</Button
										>
									{/if}
									{#if installation.status !== 'revoked'}
										<Button variant="text" size="sm" onclick={() => stop(installation, 'revoke')}
											>{$t('market.revoke')}</Button
										>
									{/if}
									<Button variant="text" size="sm" onclick={() => openLog(installation)}
										>{$t('market.log')}</Button
									>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	{#if admin && !loading}
		<DraftsSection {workspaceId} kind="package_upload" />
	{/if}
</div>

<Modal bind:open={requestOpen} title={$t('market.request')} maxWidthClass="max-w-2xl">
	<div class="space-y-4">
		{#if sortedPackages.length === 0}
			<p class="text-sm text-fg-muted">{$t('market.noPackages')}</p>
		{:else}
			<label class="block text-sm text-fg">
				<span class="mb-1 block">{$t('market.package')}</span>
				<select
					class="w-full rounded-control border border-line bg-raised px-3 py-2 text-sm"
					bind:value={digest}
					onchange={choosePackage}
				>
					<option value="">{$t('market.choosePackage')}</option>
					{#each sortedPackages as pkg (pkg.digest)}
						<option value={pkg.digest}>{pkg.package_key} {pkg.version}</option>
					{/each}
				</select>
			</label>

			{#if detail}
				<fieldset class="space-y-2">
					<legend class="text-sm text-fg">{$t('market.scope')}</legend>
					<label class="flex items-center gap-2 text-sm text-fg">
						<input type="radio" bind:group={scopeKind} value="workspace" />
						{$t('market.scopeWorkspace')}
					</label>
					<label class="flex items-center gap-2 text-sm text-fg">
						<input type="radio" bind:group={scopeKind} value="project" />
						{$t('market.scopeProject')}
					</label>
					{#if scopeKind === 'project'}
						<Input label={$t('market.projectId')} bind:value={projectId} />
					{/if}
				</fieldset>

				{#if detail.analysis.forms.length > 0}
					<fieldset class="space-y-3">
						<legend class="text-sm text-fg">{$t('market.binds')}</legend>
						{#each detail.analysis.forms as form (form.bind)}
							<div class="space-y-1 rounded-card border border-line p-3">
								<div class="text-sm font-medium text-fg">{form.bind}</div>
								{#if form.create}
									<label class="flex items-center gap-2 text-sm text-fg">
										<input type="radio" bind:group={bindKind[form.bind]} value="create" />
										{$t('market.bindCreate')}
									</label>
								{/if}
								<label class="flex items-center gap-2 text-sm text-fg">
									<input type="radio" bind:group={bindKind[form.bind]} value="existing" />
									{$t('market.bindExisting')}
								</label>
								{#if bindKind[form.bind] === 'existing'}
									<Input label={$t('market.formId')} bind:value={bindForm[form.bind]} />
								{/if}
							</div>
						{/each}
					</fieldset>
				{/if}

				{#if checked}
					{@render previewBody(checked, false, detail)}
				{/if}
			{/if}
		{/if}
	</div>
	{#snippet footer()}
		<Button variant="text" onclick={() => (requestOpen = false)}>{$t('common.cancel')}</Button>
		<Button variant="secondary" disabled={!detail || busy} onclick={check}
			>{$t('market.preview')}</Button
		>
		<Button variant="primary" disabled={!detail || !checked?.valid || busy} onclick={submitRequest}
			>{$t('market.submit')}</Button
		>
	{/snippet}
</Modal>

<Modal
	bind:open={reviewOpen}
	title={$t('market.reviewTitle', { values: { name: reviewing?.package_key ?? '' } })}
	maxWidthClass="max-w-2xl"
>
	{#if review}
		{@render previewBody(review, true, reviewDetail)}
	{:else}
		<p class="text-sm text-fg-muted">{$t('market.loading')}</p>
	{/if}
	{#snippet footer()}
		<Button variant="text" onclick={() => (reviewOpen = false)}>{$t('common.cancel')}</Button>
		{#if review?.pending}
			<Button variant="secondary" disabled={busy} onclick={reject}>{$t('market.reject')}</Button>
		{/if}
		{#if review?.valid}
			<Button variant="primary" disabled={busy} onclick={approve}>{$t('market.approve')}</Button>
		{/if}
	{/snippet}
</Modal>

<Modal
	bind:open={logOpen}
	title={$t('market.logTitle', { values: { name: logFor?.package_key ?? '' } })}
	maxWidthClass="max-w-2xl"
>
	<div class="mb-2 text-sm font-medium text-fg">{$t('market.logInvocations')}</div>
	{#if log.length === 0}
		<p class="text-sm text-fg-muted">{$t('market.logEmpty')}</p>
	{:else}
		<table class="min-w-full divide-y divide-line text-sm">
			<thead>
				<tr class="text-left text-xs text-fg-muted">
					<th class="py-2 pr-3">{$t('market.logColWhen')}</th>
					<th class="py-2 pr-3">{$t('market.logColEntry')}</th>
					<th class="py-2 pr-3">{$t('market.logColOutcome')}</th>
					<th class="py-2">{$t('market.logColDuration')}</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-line">
				{#each log as row (row.id)}
					<tr>
						<td class="py-2 pr-3 text-fg-muted">{new Date(row.created_at).toLocaleString()}</td>
						<td class="py-2 pr-3 text-fg">{row.entry}</td>
						<td class="py-2 pr-3 text-fg">{row.error_code ?? row.outcome}</td>
						<td class="py-2 text-fg-muted">{row.duration_ms} ms</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
	<p class="mt-3 text-xs text-fg-muted">{$t('market.logNote')}</p>

	<div class="mb-2 mt-6 text-sm font-medium text-fg">{$t('market.callsTitle')}</div>
	{#if calls.length === 0}
		<p class="text-sm text-fg-muted" data-market-calls-empty>{$t('market.callsEmpty')}</p>
	{:else}
		<table class="min-w-full divide-y divide-line text-sm" data-market-calls>
			<thead>
				<tr class="text-left text-xs text-fg-muted">
					<th class="py-2 pr-3">{$t('market.logColWhen')}</th>
					<th class="py-2 pr-3">{$t('market.callsColOp')}</th>
					<th class="py-2 pr-3">{$t('market.logColOutcome')}</th>
					<th class="py-2">{$t('market.logColDuration')}</th>
				</tr>
			</thead>
			<tbody class="divide-y divide-line">
				{#each calls as call (call.id)}
					<tr>
						<td class="py-2 pr-3 text-fg-muted">{new Date(call.created_at).toLocaleString()}</td>
						<td class="py-2 pr-3 text-fg">
							<div>{call.op}</div>
							<div class="text-xs text-fg-muted">
								<StatusChip tone={riskTone(call.risk)}>{$t(`market.risk.${call.risk}`)}</StatusChip>
								{call.method}
							</div>
						</td>
						<td class="py-2 pr-3 text-fg">{call.error_code ?? call.outcome}</td>
						<td class="py-2 text-fg-muted">{call.duration_ms} ms</td>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
	<p class="mt-3 text-xs text-fg-muted">{$t('market.callsNote')}</p>
</Modal>

{#snippet previewBody(preview: InstallPreview, selectable: boolean, info: PackageDetail | null)}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center gap-2 text-sm text-fg-muted" data-market-trust>
			<span>{$t('market.version', { values: { version: preview.package.version } })}</span>
			{#if info}
				<StatusChip>{tierLabel(info.tier)}</StatusChip>
				{#if info.publisher}
					<span>{$t('market.publisher', { values: { name: info.publisher.id } })}</span>
				{/if}
			{/if}
			{#if preview.package.custom_code}
				<span class="text-warning">{$t('market.customCode')}</span>
			{/if}
		</div>

		{#if !preview.valid}
			<div class="rounded-card border border-danger p-3 text-sm text-danger">
				<div class="font-medium">{$t('market.problems')}</div>
				<ul class="mt-1 list-disc pl-5">
					{#each preview.errors as problem (problem.code + problem.path)}
						<li>{problem.code} {problem.path} {problem.message}</li>
					{/each}
				</ul>
			</div>
		{/if}

		{#if preview.installation && preview.installation.changed_grants.length > 0}
			<div class="rounded-card border border-warning p-3 text-sm text-fg">
				<div>{$t('market.changedGrants')}</div>
				<ul class="mt-1 list-disc pl-5">
					{#each preview.installation.changed_grants as grant (grant.capability + JSON.stringify(grant.selector))}
						<li>{words(grant.capability)}</li>
					{/each}
				</ul>
			</div>
		{/if}

		<div>
			<div class="text-sm font-medium text-fg">{$t('market.asks')}</div>
			{#if preview.requests.length === 0}
				<p class="mt-1 text-sm text-fg-muted">{$t('market.asksNothing')}</p>
			{:else}
				<div class="mt-2 space-y-4" data-market-areas>
					{#each groupRequests(preview.requests, counts) as group (group.area)}
						<section class="rounded-card border border-line" data-market-area={group.area}>
							<header class="border-b border-line bg-nav px-3 py-2">
								<div class="flex flex-wrap items-baseline justify-between gap-2">
									<span class="text-sm font-medium text-fg">{areaName(group.area)}</span>
									{#if group.ops !== null}
										<span class="text-xs text-fg-muted"
											>{$t('market.areaOps', { values: { count: group.ops } })}</span
										>
									{/if}
								</div>
								{#if areaDescription(group.area)}
									<p class="mt-0.5 text-xs text-fg-muted">{areaDescription(group.area)}</p>
								{/if}
							</header>
							{#each group.tiers as tier (tier.tier)}
								<div class="px-3 py-2" data-market-tier={tier.tier}>
									<div class="mb-1 text-xs font-medium uppercase tracking-wider text-fg-muted">
										{$t(`market.riskTier.${tier.tier}`)}
									</div>
									<ul class="space-y-2">
										{#each tier.items as { index, request } (index)}
											<li class="flex items-start gap-3 rounded-card border border-line p-3">
												{#if selectable}
													<Checkbox
														checked={ticked[index] === true}
														onchange={(event) => void toggle(index, event.currentTarget.checked)}
														aria-label={words(request.capability)}
													/>
												{/if}
												<div class="text-sm">
													<div class="text-fg">
														{words(request.capability)}
														{#if request.new && preview.existing.installation_id}
															<StatusChip tone="warning">{$t('market.newRequest')}</StatusChip>
														{/if}
													</div>
													<div class="text-xs text-fg-muted">
														<StatusChip tone={riskTone(request.risk)}
															>{$t(`market.risk.${request.risk}`)}</StatusChip
														>
														{#if request.selector.form}
															· {$t('market.onForm', { values: { form: request.selector.form } })}
														{:else if request.form_bind}
															· {$t('market.bindNote')}
														{:else}
															· {$t(
																(preview.installation?.scope.kind ?? scopeKind) === 'project'
																	? 'market.wholeProject'
																	: 'market.wholeWorkspace'
															)}
														{/if}
													</div>
												</div>
											</li>
										{/each}
									</ul>
								</div>
							{/each}
						</section>
					{/each}
				</div>
				{#if selectable}
					<p class="mt-2 text-xs text-fg-muted">{$t('market.approveSubset')}</p>
				{/if}
			{/if}
		</div>

		{#if preview.requests.length > 0}
			<div class="text-sm text-fg" data-market-data-access>
				<div class="font-medium">{$t('market.dataAccess')}</div>
				<ul class="mt-1 space-y-1 text-fg-muted">
					{#each dataAccess(preview.requests, words) as row, at (at)}
						<li>
							<span class="text-fg"
								>{row.target
									? $t('market.onForm', { values: { form: row.target } })
									: $t(
											(preview.installation?.scope.kind ?? scopeKind) === 'project'
												? 'market.wholeProject'
												: 'market.wholeWorkspace'
										)}</span
							>
							{#if row.reads.length}<div>
									{$t('market.dataReads', { values: { list: row.reads.join(', ') } })}
								</div>{/if}
							{#if row.writes.length}<div>
									{$t('market.dataWrites', { values: { list: row.writes.join(', ') } })}
								</div>{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/if}

		{#if info && publicPages(info).length > 0}
			<div class="rounded-card border border-warning p-3 text-sm text-fg" data-market-public>
				<div class="font-medium">{$t('market.publicExposure')}</div>
				<p class="mt-1 text-fg-muted">
					{$t('market.publicPages', { values: { list: publicPages(info).join(', ') } })}
				</p>
			</div>
		{/if}

		<div class="text-sm text-fg">
			<div class="font-medium">{$t('market.creates')}</div>
			<ul class="mt-1 space-y-1 text-fg-muted">
				{#if preview.creates.forms.length}
					<li>
						{$t('market.createsForms', { values: { list: createdList(preview.creates.forms) } })}
					</li>
				{/if}
				{#if preview.creates.layouts.length}
					<li>
						{$t('market.createsLayouts', {
							values: { list: createdList(preview.creates.layouts) }
						})}
					</li>
				{/if}
				{#if preview.creates.placements.length}
					<li>
						{$t('market.createsPlacements', {
							values: { list: createdList(preview.creates.placements.map((p) => p.slot_id)) }
						})}
					</li>
				{/if}
				{#if preview.creates.pages.length}
					<li>
						{$t('market.createsPages', { values: { list: createdList(preview.creates.pages) } })}
					</li>
				{/if}
				{#if !preview.creates.forms.length && !preview.creates.layouts.length && !preview.creates.pages.length}
					<li>{$t('market.createsNothing')}</li>
				{/if}
			</ul>
		</div>
	</div>
{/snippet}
