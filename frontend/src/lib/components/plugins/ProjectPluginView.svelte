<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { locale, t } from 'svelte-i18n';
	import PluginFrameHost from './PluginFrameHost.svelte';
	import PluginPreferenceMenu from './PluginPreferenceMenu.svelte';
	import type { PluginContributionItem } from '#lib/api/plugin-packages.js';
	import {
		appHref,
		currentTheme,
		enabledRoutes,
		findByRoute,
		frameHeights,
		frameKey,
		frameVerdict,
		itemKey,
		layoutSlot,
		loadEntry,
		navigationHref,
		projectDiscovery,
		projectHref,
		projectName,
		sameFrameIdentity,
		verdictReasons,
		type DiscoveryState
	} from '#lib/plugins/contributions.js';
	import { createExtensionTransport, hostContext } from '#lib/plugins/host-methods.js';

	let {
		workspaceId,
		projectId,
		pluginKey,
		route,
		routeState,
		from
	}: {
		workspaceId: string;
		projectId: string;
		pluginKey: string;
		route: string;
		routeState: string | null;
		/** A validated in-project return path, or null for the project page. */
		from: string | null;
	} = $props();

	type Opened = { item: PluginContributionItem; html: string };
	const backHref = $derived(from ?? projectHref(workspaceId, projectId));

	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });
	let opened = $state.raw<Opened | null>(null);
	let phase = $state<'loading' | 'ready' | 'failed' | 'unavailable'>('loading');
	let notice = $state('');
	let name = $state('');
	let theme = $state<'light' | 'dark'>(currentTheme());
	let run = 0;
	let inflight = false;

	const found = $derived(findByRoute(discovery.items, pluginKey, route));
	const tabs = $derived(layoutSlot(discovery.items, 'project.view'));
	const context = $derived(
		opened
			? hostContext(opened.item, {
					projectName: name,
					routeState,
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
					// Always the current discovery, never the manifest.
					enabledRoutes: () => enabledRoutes(discovery.items, pluginKey),
					navigate: async (target) => {
						await goto(navigationHref(target, { workspaceId, projectId, key: pluginKey, from }));
					},
					onStale: () => void projectDiscovery(projectId).refresh()
				})
			: undefined
	);

	$effect(() => {
		const store = projectDiscovery(projectId);
		const stop = store.subscribe((value) => (discovery = value));
		const release = store.retain();
		return () => {
			stop();
			release();
		};
	});

	$effect(() => {
		if (!opened) return;
		return projectDiscovery(projectId).holdBusy();
	});

	$effect(() => {
		const current = discovery;
		const target = found;
		untrack(() => reconcile(current, target));
	});

	onMount(() => {
		const observer = new MutationObserver(() => (theme = currentTheme()));
		observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
		return () => observer.disconnect();
	});

	function reconcile(current: DiscoveryState, target: PluginContributionItem | undefined) {
		if (opened) {
			const verdict = frameVerdict(current, opened.item);
			if (verdict === 'ok') return;
			run++;
			inflight = false;
			opened = null;
			notice = verdictReasons[verdict];
			if (verdict === 'error') {
				phase = 'failed';
				return;
			}
			phase = 'loading';
		}
		if (phase === 'failed' && target && current.status !== 'off' && current.status !== 'denied')
			return;
		if (current.status === 'loading') {
			phase = 'loading';
			return;
		}
		if (current.status === 'off' || current.status === 'denied' || !target) {
			run++;
			inflight = false;
			phase = 'unavailable';
			return;
		}
		if (!inflight) void open(target);
	}

	async function open(target: PluginContributionItem) {
		const mine = ++run;
		inflight = true;
		phase = 'loading';
		const [entry, label] = await Promise.all([
			loadEntry(projectId, target, {}),
			projectName(projectId)
		]);
		if (mine !== run) return;
		inflight = false;
		name = label;
		if (!entry.ok) {
			phase = entry.reason === 'unavailable' ? 'unavailable' : 'failed';
			notice = entry.reason === 'unavailable' ? '' : 'plugins.host.loadFailed';
			return;
		}
		if (!sameFrameIdentity(entry.item, target)) {
			// Discovery was stale. Only continue when a fresh discovery agrees with the entry.
			await projectDiscovery(projectId).refresh();
			if (mine !== run) return;
			const latest = findByRoute(discovery.items, pluginKey, route);
			if (!latest || !sameFrameIdentity(latest, entry.item)) {
				phase = 'failed';
				notice = 'plugins.versionChanged';
				return;
			}
		}
		notice = '';
		opened = { item: entry.item, html: entry.html };
		phase = 'ready';
	}

	function onClose(reason: string) {
		opened = null;
		if (reason === '') {
			void goto(backHref);
			return;
		}
		run++;
		inflight = false;
		phase = 'failed';
		notice = reason;
		void projectDiscovery(projectId).refresh();
	}

	function retry() {
		notice = '';
		phase = 'loading';
		reconcile(discovery, found);
	}
</script>

{#if phase === 'unavailable'}
	<section class="space-y-3" data-plugin-unavailable>
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t('plugins.host.unavailable')}
		</h1>
		<p class="text-sm text-slate-600 dark:text-slate-300">{$t('plugins.host.unavailableHint')}</p>
		<a href={backHref} class="text-sm text-blue-700 underline dark:text-blue-300"
			>{$t(from ? 'plugins.host.back' : 'plugins.host.backToProject')}</a
		>
	</section>
{:else}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<a href={backHref} class="text-sm text-blue-700 underline dark:text-blue-300"
				>{$t(from ? 'plugins.host.back' : 'plugins.host.backToProject')}</a
			>
			<PluginPreferenceMenu {projectId} items={discovery.items} />
		</div>
		{#if tabs.inline.length + tabs.more.length > 0}
			<nav aria-label={$t('plugins.apps.tabsLabel')} class="flex flex-wrap items-center gap-2">
				{#each tabs.inline as item (itemKey(item))}
					<a
						href={appHref(workspaceId, projectId, item.key, item.contribution.route ?? '', {
							from
						})}
						aria-current={item.key === pluginKey && item.contribution.route === route
							? 'page'
							: undefined}
						class="rounded-md border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50 focus-visible:outline-blue-600 aria-[current=page]:bg-slate-100 aria-[current=page]:font-semibold dark:border-slate-600 dark:hover:bg-slate-800 dark:aria-[current=page]:bg-slate-800"
						>{item.contribution.label}</a
					>
				{/each}
				{#if tabs.more.length}
					<details class="relative">
						<summary class="cursor-pointer rounded-md px-2 py-1.5 text-sm underline"
							>{$t('plugins.host.more')}</summary
						>
						<ul
							class="absolute z-20 mt-1 min-w-40 space-y-1 rounded-md border border-slate-300 bg-white p-2 text-sm shadow dark:border-slate-600 dark:bg-slate-900"
						>
							{#each tabs.more as item (itemKey(item))}
								<li>
									<a
										class="block underline"
										href={appHref(workspaceId, projectId, item.key, item.contribution.route ?? '', {
											from
										})}>{item.contribution.label}</a
									>
								</li>
							{/each}
						</ul>
					</details>
				{/if}
			</nav>
		{/if}
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{found?.contribution.label ?? pluginKey}
		</h1>
		{#if notice && phase !== 'ready'}
			<p class="text-sm text-slate-600 dark:text-slate-300" role="status">{$t(notice)}</p>
		{/if}
		{#if phase === 'loading'}
			<p class="text-sm text-slate-600 dark:text-slate-300" role="status">
				{$t('plugins.host.loading')}
			</p>
		{:else if phase === 'failed'}
			<button
				type="button"
				onclick={retry}
				class="rounded-md border border-slate-300 px-3 py-1.5 text-sm dark:border-slate-600"
				>{$t('plugins.host.retry')}</button
			>
		{/if}
		{#if opened && transport && context}
			{#key frameKey(opened.item)}
				<div class="overflow-hidden rounded-md border border-slate-300 dark:border-slate-600">
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
						minHeight={frameHeights.view.min}
						maxHeight={frameHeights.view.max}
						{onClose}
					/>
				</div>
			{/key}
		{/if}
	</div>
{/if}
