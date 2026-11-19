<script lang="ts">
	import { getContext, onMount, tick, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { locale, t } from 'svelte-i18n';
	import CreatorPluginLink from './CreatorPluginLink.svelte';
	import PluginFrameHost from './PluginFrameHost.svelte';
	import PluginPreferenceMenu from './PluginPreferenceMenu.svelte';
	import type { PluginContributionItem } from '#lib/api/plugin-packages.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import {
		currentTheme,
		enabledRoutes,
		formDiscovery,
		frameHeights,
		frameKey,
		frameVerdict,
		itemKey,
		layoutSlot,
		loadEntry,
		navigationHref,
		projectDiscovery,
		projectName,
		safeFrom,
		sameFrameIdentity,
		verdictReasons,
		type DiscoveryState
	} from '#lib/plugins/contributions.js';
	import { createExtensionTransport, hostContext } from '#lib/plugins/host-methods.js';

	let {
		workspaceId,
		projectId,
		form
	}: {
		workspaceId: string;
		projectId: string;
		form: { id: string; key: string } | null;
	} = $props();

	type Opened = { item: PluginContributionItem; html: string };
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	const drawerId = $props.id();

	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });
	let active = $state.raw<PluginContributionItem | null>(null);
	let opened = $state.raw<Opened | null>(null);
	let phase = $state<'closed' | 'loading' | 'ready' | 'failed'>('closed');
	let notice = $state('');
	let name = $state('');
	let theme = $state<'light' | 'dark'>(currentTheme());
	let closeButton: HTMLButtonElement | undefined = $state();
	let launch: HTMLElement | null = null;
	let run = 0;

	const formId = $derived(form?.id ?? null);
	const layout = $derived(layoutSlot(discovery.items, 'form.list.toolbar'));
	const context = $derived(
		opened
			? hostContext(opened.item, {
					projectName: name,
					locale: $locale ?? 'zh',
					theme
				})
			: undefined
	);
	const transport = $derived(
		opened
			? createExtensionTransport({
					projectId,
					item: opened.item,
					// Routes come from the current project discovery, never the manifest.
					enabledRoutes: () =>
						enabledRoutes(projectDiscovery(projectId).snapshot().items, opened?.item.key ?? ''),
					navigate: async (target) => {
						const from = safeFrom(page.url.pathname + page.url.search, workspaceId, projectId);
						await goto(
							navigationHref(target, {
								workspaceId,
								projectId,
								key: opened?.item.key ?? '',
								from
							})
						);
					},
					onStale: () => void formStore()?.refresh()
				})
			: undefined
	);

	function formStore() {
		return formId ? formDiscovery(projectId, formId) : null;
	}

	$effect(() => {
		const id = formId;
		untrack(() => closeDrawer('', false));
		discovery = { status: 'loading', items: [] };
		if (!id) return;
		const store = formDiscovery(projectId, id);
		const stop = store.subscribe((value) => (discovery = value));
		const release = store.retain();
		return () => {
			stop();
			release();
		};
	});

	$effect(() => {
		if (!active) return;
		const release = [formStore()?.holdBusy(), projectDiscovery(projectId).retain()];
		return () => release.forEach((fn) => fn?.());
	});

	$effect(() => {
		const current = discovery;
		untrack(() => {
			const watched = opened?.item ?? active;
			if (!watched) return;
			const verdict = frameVerdict(current, watched);
			if (verdict !== 'ok') closeDrawer(verdictReasons[verdict], true);
		});
	});

	onMount(() => {
		const observer = new MutationObserver(() => (theme = currentTheme()));
		observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
		return () => observer.disconnect();
	});

	function closeDrawer(message = '', restoreFocus = true) {
		run++;
		active = null;
		opened = null;
		phase = 'closed';
		notice = message;
		const trigger = launch;
		launch = null;
		if (restoreFocus) void tick().then(() => trigger?.isConnected && trigger.focus());
	}

	async function openDrawer(item: PluginContributionItem, trigger: HTMLElement) {
		if (!formId) return;
		if (active && itemKey(active) === itemKey(item)) {
			closeDrawer();
			return;
		}
		closeDrawer('', false);
		launch = trigger;
		const mine = ++run;
		active = item;
		phase = 'loading';
		const [entry, label] = await Promise.all([
			loadEntry(projectId, item, { formId }),
			projectName(projectId)
		]);
		if (mine !== run) return;
		name = label;
		if (!entry.ok) {
			closeDrawer(
				entry.reason === 'unavailable' ? 'plugins.accessChanged' : 'plugins.host.loadFailed'
			);
			void formStore()?.refresh();
			return;
		}
		if (!sameFrameIdentity(entry.item, item)) {
			closeDrawer('plugins.versionChanged');
			void formStore()?.refresh();
			return;
		}
		opened = { item: entry.item, html: entry.html };
		phase = 'ready';
		await tick();
		if (mine === run) closeButton?.focus();
	}

	function onClose(reason: string) {
		closeDrawer(reason);
		if (reason !== '') void formStore()?.refresh();
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') closeDrawer();
	}
</script>

{#if form && (discovery.status === 'ready' || discovery.status === 'error') && (discovery.items.length > 0 || canManage?.() || notice)}
	<div class="inline-flex flex-wrap items-center gap-2" data-plugin-slot="form.list.toolbar">
		{#each layout.inline as item (itemKey(item))}
			<button
				type="button"
				class="inline-flex h-8 items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium text-slate-700 hover:bg-slate-50 focus-visible:outline-blue-600 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800"
				data-plugin-contribution={item.contribution.id}
				aria-expanded={active ? itemKey(active) === itemKey(item) : false}
				aria-controls={active && itemKey(active) === itemKey(item) ? drawerId : undefined}
				onclick={(event) => void openDrawer(item, event.currentTarget)}
				>{item.contribution.label}</button
			>
		{/each}
		{#if layout.more.length}
			<details class="relative">
				<summary
					class="inline-flex h-8 cursor-pointer items-center rounded-md border border-slate-300 px-2.5 text-xs font-medium dark:border-slate-700"
					>{$t('plugins.host.more')}</summary
				>
				<ul
					class="absolute right-0 z-20 mt-1 min-w-44 space-y-1 rounded-md border border-slate-300 bg-white p-2 text-sm shadow dark:border-slate-600 dark:bg-slate-900"
				>
					{#each layout.more as item (itemKey(item))}
						<li>
							<button
								type="button"
								class="block w-full text-left underline"
								data-plugin-contribution={item.contribution.id}
								onclick={(event) => {
									const details = event.currentTarget.closest('details');
									if (details) details.open = false;
									void openDrawer(item, event.currentTarget);
								}}>{item.contribution.label}</button
							>
						</li>
					{/each}
				</ul>
			</details>
		{/if}
		<PluginPreferenceMenu {projectId} items={discovery.items} />
		<CreatorPluginLink {workspaceId} {projectId} slot="form.list.toolbar" formKey={form.key} />
		{#if notice && phase === 'closed'}
			<span class="text-xs text-slate-600 dark:text-slate-300" role="status">{$t(notice)}</span>
		{/if}
	</div>
{/if}

{#if active}
	<div
		id={drawerId}
		role="dialog"
		tabindex="-1"
		aria-label={$t('plugins.host.openDrawer', { values: { label: active.contribution.label } })}
		class="fixed inset-y-0 right-0 z-40 flex w-full max-w-xl flex-col border-l border-slate-300 bg-white text-slate-900 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
		onkeydown={onKeydown}
	>
		<div
			class="flex items-center justify-between gap-2 border-b border-slate-200 p-3 dark:border-slate-700"
		>
			<h3 class="text-sm font-semibold">{active.name} · {active.contribution.label}</h3>
			<button
				type="button"
				bind:this={closeButton}
				onclick={() => closeDrawer()}
				class="rounded-md px-2 py-1 text-sm underline focus-visible:outline-blue-600"
				>{$t('plugins.host.closeDrawer')}</button
			>
		</div>
		<div class="min-h-0 flex-1 overflow-auto">
			{#if phase === 'loading'}
				<p class="p-3 text-sm text-slate-600 dark:text-slate-300" role="status">
					{$t('plugins.host.loading')}
				</p>
			{/if}
			{#if opened && transport && context}
				{#key frameKey(opened.item)}
					<PluginFrameHost
						html={opened.html}
						webSha256={opened.item.web_sha256}
						bridge={2}
						grants={[...opened.item.grants]}
						{context}
						{transport}
						title={$t('plugins.host.frameTitle', {
							values: { name: opened.item.name, label: opened.item.contribution.label }
						})}
						minHeight={frameHeights.panel.min}
						maxHeight={frameHeights.panel.max}
						{onClose}
					/>
				{/key}
			{/if}
		</div>
	</div>
{/if}
