<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import CreatorPanelPreview from './CreatorPanelPreview.svelte';
	import { verifyPanelHtml } from '#lib/plugins/bridge.js';
	import Button from '#lib/components/Button.svelte';
	import {
		pluginLibraryApi,
		type LibraryProject,
		type PersonalPluginPackage
	} from '#lib/api/plugin-packages.js';
	import { apiClient } from '#lib/api/client.js';
	import { onMount } from 'svelte';

	let {
		item,
		projects,
		onChanged
	}: {
		item: PersonalPluginPackage;
		projects: LibraryProject[];
		onChanged: () => Promise<void>;
	} = $props();
	let open = $state(false);
	let html = $state<string | null>(null);
	let panelState = $state<'loading' | 'error' | null>(null);
	let panelLoad = 0;
	const placement = $derived(item.manifest.ui.contributions?.[0]);
	const slot = $derived(
		placement?.slot ??
			(item.manifest.ui.slot === 'form.record.actions'
				? 'form.record.actions'
				: 'form.record.detail')
	);
	let quantity = $state<number | undefined>();
	let unitPrice = $state<number | undefined>();
	let amount = $state<number | undefined>();
	let projectId = $state('');
	let pending = $state<'preview' | 'attach' | 'revoke' | null>(null);
	let error = $state('');
	let notice = $state('');
	let confirmRemoval = $state<string | null>(null);
	let current = true;
	const canConnect = $derived(
		projects.filter(
			(project) =>
				!item.project_links.some(
					(link) =>
						link.project_id === project.id &&
						(link.status === 'attached' || link.status === 'pending')
				)
		)
	);
	const target = $derived(canConnect.find((project) => project.id === projectId));

	onMount(() => {
		const signal = apiClient.authSessionSignal();
		const abort = () => {
			current = false;
		};
		signal.addEventListener('abort', abort, { once: true });
		return () => {
			abort();
			signal.removeEventListener('abort', abort);
		};
	});

	const formatCents = (cents: number, language: string) =>
		new Intl.NumberFormat(language, { minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(
			cents / 100
		);

	async function togglePanel() {
		open = !open;
		const request = ++panelLoad;
		html = null;
		if (!open) return;
		panelState = 'loading';
		const result = await pluginLibraryApi.panel(item.id);
		if (!current || request !== panelLoad) return;
		try {
			if (result.code === 0 && result.data && (await verifyPanelHtml(result.data))) {
				if (current && request === panelLoad) {
					html = result.data.html;
					panelState = null;
				}
				return;
			}
		} catch {
			/* Integrity verification can be unavailable. */
		}
		if (current && request === panelLoad) panelState = 'error';
	}

	async function calculate(): Promise<number | null> {
		if (pending || quantity === undefined || unitPrice === undefined) return null;
		const cents = Math.round(unitPrice * 100);
		if (
			!Number.isSafeInteger(quantity) ||
			quantity < 1 ||
			quantity > 1_000_000 ||
			!Number.isSafeInteger(cents) ||
			cents < 0 ||
			cents > 1_000_000_000
		)
			return null;
		pending = 'preview';
		error = notice = '';
		amount = undefined;
		const result = await pluginLibraryApi.preview(item.id, quantity, cents);
		if (!current) return null;
		pending = null;
		if (
			result.code === 0 &&
			result.data &&
			Number.isSafeInteger(result.data.amount_cents) &&
			result.data.amount_cents >= 0 &&
			result.data.amount_cents <= 1_000_000_000_000
		) {
			amount = result.data.amount_cents;
			return amount;
		}
		error = 'plugins.library.previewFailed';
		return null;
	}

	function preview(event: SubmitEvent) {
		event.preventDefault();
		void calculate();
	}

	async function attach() {
		if (!target || pending) return;
		pending = 'attach';
		error = notice = '';
		const result = await pluginLibraryApi.connect(target.id, item.id);
		if (!current) return;
		if (result.code === 0 && result.data) {
			notice =
				result.data.status === 'attached'
					? 'plugins.library.attachedNotice'
					: 'plugins.library.pendingNotice';
			projectId = '';
			await onChanged();
		} else error = 'plugins.library.attachFailed';
		pending = null;
	}

	async function revoke(project: string) {
		if (pending || confirmRemoval !== project) return;
		pending = 'revoke';
		error = notice = '';
		const result = await pluginLibraryApi.revoke(project, item.id);
		if (!current) return;
		if (result.code === 0) {
			confirmRemoval = null;
			notice = 'plugins.library.removedNotice';
			await onChanged();
		} else error = 'plugins.library.removeFailed';
		pending = null;
	}
</script>

<article
	class="space-y-4 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
>
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div class="min-w-0 space-y-1">
			<h3 class="break-words font-semibold">
				{item.manifest.name} <span class="text-sm font-normal">v{item.manifest.version}</span>
			</h3>
			<p class="text-sm text-slate-600 dark:text-slate-300">
				{item.manifest.key} · {item.manifest.publisher}
			</p>
		</div>
		{#if !item.archived_at}
			<Button variant="secondary" onclick={togglePanel} aria-expanded={open}>
				{$t(open ? 'plugins.library.closePreview' : 'plugins.library.openPreview')}
			</Button>
		{/if}
	</div>

	{#if open && !item.archived_at}
		<form class="space-y-3 border-l-2 border-blue-500 pl-3" onsubmit={preview}>
			<div class="grid max-w-lg gap-3 sm:grid-cols-2">
				<label class="grid gap-1 text-sm">
					{$t('plugins.library.quantity')}
					<input
						type="number"
						min="1"
						max="1000000"
						step="1"
						required
						bind:value={quantity}
						disabled={pending === 'preview'}
						oninput={() => {
							amount = undefined;
						}}
						class="w-full rounded-md border border-slate-300 bg-transparent px-3 py-2 dark:border-slate-600"
					/>
				</label>
				<label class="grid gap-1 text-sm">
					{$t('plugins.library.unitPrice')}
					<input
						type="number"
						min="0"
						max="10000000"
						step="0.01"
						required
						bind:value={unitPrice}
						disabled={pending === 'preview'}
						oninput={() => {
							amount = undefined;
						}}
						class="w-full rounded-md border border-slate-300 bg-transparent px-3 py-2 dark:border-slate-600"
					/>
				</label>
			</div>
			<div class="flex flex-wrap items-center gap-3">
				<Button
					type="submit"
					loading={pending === 'preview'}
					disabled={pending !== null && pending !== 'preview'}
					>{$t('plugins.library.runPreview')}</Button
				>
				{#if amount !== undefined}<output aria-live="polite" class="font-semibold tabular-nums"
						>{$t('plugins.library.previewAmount', {
							values: { amount: formatCents(amount, $locale || 'en') }
						})}</output
					>{/if}
			</div>
		</form>
		{#if html}
			<CreatorPanelPreview {html} {slot} placementId={placement?.id} onPreview={calculate} />
		{:else if panelState === 'loading'}
			<p role="status" class="text-sm">{$t('plugins.loadingPanel')}</p>
		{:else if panelState === 'error'}
			<p role="alert" class="text-sm text-red-800 dark:text-red-200">{$t('plugins.loadFailed')}</p>
		{/if}
	{/if}

	<div class="space-y-3 border-t border-slate-200 pt-3 dark:border-slate-700">
		<h4 class="text-sm font-semibold">{$t('plugins.library.projectUse')}</h4>
		{#if item.project_links.length}
			<ul class="divide-y divide-slate-200 dark:divide-slate-700">
				{#each item.project_links as link (link.project_id)}
					{@const project = projects.find((entry) => entry.id === link.project_id)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
						<div class="min-w-0">
							<p class="break-words font-medium">
								{project
									? `${project.workspace_name} / ${project.name}`
									: $t('plugins.library.unavailableProject')}
							</p>
							<p>{$t(`plugins.library.linkStatus.${link.active ? 'enabled' : link.status}`)}</p>
						</div>
						<div class="flex flex-wrap items-center gap-2">
							{#if project?.can_manage && link.status === 'attached'}
								<a
									href={`/workspace/${encodeURIComponent(project.workspace_id)}/projects/${encodeURIComponent(project.id)}/settings/plugins`}
									class="text-blue-700 underline dark:text-blue-300"
									>{$t('plugins.library.manageInProject')}</a
								>
							{/if}
							{#if confirmRemoval === link.project_id}
								<p class="text-sm">
									{$t(
										link.status === 'pending'
											? 'plugins.library.cancelQuestion'
											: 'plugins.library.removeQuestion'
									)}
								</p>
								<Button
									variant="danger"
									size="sm"
									loading={pending === 'revoke'}
									onclick={() => revoke(link.project_id)}>{$t('plugins.library.confirm')}</Button
								>
								<Button
									variant="secondary"
									size="sm"
									disabled={!!pending}
									onclick={() => {
										confirmRemoval = null;
									}}>{$t('common.cancel')}</Button
								>
							{:else if link.status === 'attached' || link.status === 'pending'}
								<Button
									variant="secondary"
									size="sm"
									disabled={!!pending}
									onclick={() => {
										confirmRemoval = link.project_id;
									}}
								>
									{$t(
										link.status === 'pending'
											? 'plugins.library.cancelRequest'
											: 'plugins.library.remove'
									)}
								</Button>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="text-sm text-slate-600 dark:text-slate-300">{$t('plugins.library.noProjectUse')}</p>
		{/if}
		{#if canConnect.length && !item.archived_at}
			<div class="flex flex-wrap items-end gap-2">
				<label class="grid min-w-0 gap-1 text-sm">
					{$t('plugins.library.targetProject')}
					<select
						bind:value={projectId}
						disabled={!!pending}
						class="w-full rounded-md border border-slate-300 bg-transparent px-3 py-2 dark:border-slate-600"
					>
						<option value="">{$t('plugins.library.chooseProject')}</option>
						{#each canConnect as project (project.id)}
							<option value={project.id}>{project.workspace_name} / {project.name}</option>
						{/each}
					</select>
				</label>
				<Button onclick={attach} loading={pending === 'attach'} disabled={!target || !!pending}
					>{$t(
						target && !target.can_manage
							? 'plugins.library.requestAccess'
							: 'plugins.library.attach'
					)}</Button
				>
			</div>
		{/if}
	</div>
	{#if error}<p role="alert" class="text-sm text-red-800 dark:text-red-200">{$t(error)}</p>{/if}
	{#if notice}<p role="status" class="text-sm">{$t(notice)}</p>{/if}
</article>
