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
		itemKey,
		layoutSlot,
		loadEntry,
		navigationHref,
		projectDiscovery,
		projectHref,
		projectName,
		type DiscoveryState
	} from '#lib/plugins/contributions.js';
	import { createExtensionTransport, hostContext } from '#lib/plugins/host-methods.js';
	import {
		closeView,
		initialProjectView,
		openView,
		reconcileView,
		retireFrame,
		retryView,
		type ProjectViewDeps,
		type ProjectViewState
	} from '#lib/plugins/project-view.js';

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

	const backHref = $derived(from ?? projectHref(workspaceId, projectId));

	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });
	// The view's state machine (project-view.ts) mutates this object; it is a deep $state proxy.
	const view = $state<ProjectViewState>(initialProjectView());
	let theme = $state<'light' | 'dark'>(currentTheme());

	const found = $derived(findByRoute(discovery.items, pluginKey, route));
	const tabs = $derived(layoutSlot(discovery.items, 'project.view'));
	const opened = $derived(view.opened);
	const context = $derived(
		opened
			? hostContext(opened.item, {
					projectName: view.name,
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
						// A frame that is no longer shown does not navigate anymore.
						if (view.opened?.token !== opened.token) return;
						await goto(navigationHref(target, { workspaceId, projectId, key: pluginKey, from }));
					},
					onStale: () => void projectDiscovery(projectId).refresh()
				})
			: undefined
	);
	const deps: ProjectViewDeps = {
		loadEntry: (target) => loadEntry(projectId, target, {}),
		projectName: () => projectName(projectId),
		refreshDiscovery: () => projectDiscovery(projectId).refresh(),
		latest: () => findByRoute(discovery.items, pluginKey, route)
	};

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
		const next = reconcileView(view, current, target);
		if (next) void openView(view, next, deps);
	}

	function onClose(reason: string) {
		if (closeView(view, reason) === 'back') {
			void goto(backHref);
			return;
		}
		void projectDiscovery(projectId).refresh();
	}

	/**
	 * The host of `token` retired its frame while this view was being left (route change, re-key),
	 * and the view is live again because the navigation came back before the removal finished.
	 * The host only reports this once it runs again, so the view reopens with a fresh entry.
	 */
	function retiredFor(token: number) {
		return () => {
			if (retireFrame(view, token)) reconcile(discovery, found);
		};
	}

	function retry() {
		const next = retryView(view, discovery, found);
		if (next) void openView(view, next, deps);
	}
</script>

{#if view.phase === 'unavailable'}
	<section class="space-y-3" data-plugin-unavailable>
		<h1 class="text-lg font-semibold text-fg">
			{$t('plugins.host.unavailable')}
		</h1>
		<p class="text-sm text-fg-muted">{$t('plugins.host.unavailableHint')}</p>
		<a href={backHref} class="text-sm text-fg underline"
			>{$t(from ? 'plugins.host.back' : 'plugins.host.backToProject')}</a
		>
	</section>
{:else}
	<div class="space-y-4">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<a href={backHref} class="text-sm text-fg underline"
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
						class="rounded-control border border-input px-3 py-1.5 text-sm hover:bg-surface aria-[current=page]:bg-selected aria-[current=page]:font-semibold"
						>{item.contribution.label}</a
					>
				{/each}
				{#if tabs.more.length}
					<details class="relative">
						<summary class="cursor-pointer rounded-control px-2 py-1.5 text-sm underline"
							>{$t('plugins.host.more')}</summary
						>
						<ul
							class="absolute z-20 mt-1 min-w-40 space-y-1 rounded-float border border-overlay-line bg-overlay p-2 text-sm shadow-overlay"
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
		<h1 class="text-lg font-semibold text-fg">
			{found?.contribution.label ?? pluginKey}
		</h1>
		{#if view.notice && view.phase !== 'ready'}
			<p class="text-sm text-fg-muted" role="status">{$t(view.notice)}</p>
		{/if}
		{#if view.phase === 'loading'}
			<p class="text-sm text-fg-muted" role="status">
				{$t('plugins.host.loading')}
			</p>
		{:else if view.phase === 'failed'}
			<button
				type="button"
				onclick={retry}
				class="rounded-control border border-input px-3 py-1.5 text-sm"
				>{$t('plugins.host.retry')}</button
			>
		{/if}
		{#if opened && transport && context}
			<!-- One frame per open: a later open never reuses a host that is still being removed. -->
			{#key opened.token}
				<div class="overflow-hidden rounded-card border border-line">
					<PluginFrameHost
						html={opened.html}
						webSha256={opened.item.web_sha256}
						grants={[...opened.item.grants]}
						{context}
						{transport}
						title={$t('plugins.host.frameTitle', {
							values: { name: opened.item.name, label: opened.item.contribution.label }
						})}
						minHeight={frameHeights.view.min}
						maxHeight={frameHeights.view.max}
						{onClose}
						onRetired={retiredFor(opened.token)}
					/>
				</div>
			{/key}
		{/if}
	</div>
{/if}
