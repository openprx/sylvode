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
		frameVerdict,
		itemKey,
		layoutSlot,
		loadEntry,
		navigationHref,
		projectDiscovery,
		projectName,
		safeFrom,
		verdictReasons,
		type DiscoveryState
	} from '#lib/plugins/contributions.js';
	import {
		closeSlot,
		initialSlot,
		openSlot,
		retireSlot,
		type SlotDeps,
		type SlotState
	} from '#lib/plugins/frame-slot.js';
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

	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
	const drawerId = $props.id();

	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });
	// The drawer's state machine (frame-slot.ts) mutates this object; it is a deep $state proxy.
	const slot = $state<SlotState<PluginContributionItem>>(initialSlot());
	let theme = $state<'light' | 'dark'>(currentTheme());
	let closeButton: HTMLButtonElement | undefined = $state();
	let launch: HTMLElement | null = null;

	const active = $derived(slot.active);
	const opened = $derived(
		slot.frame?.item
			? { token: slot.frame.token, item: slot.frame.item, html: slot.frame.html }
			: null
	);
	const phase = $derived(active ? (slot.frame ? 'ready' : 'loading') : 'closed');
	const notice = $derived(slot.notice);
	const formId = $derived(form?.id ?? null);
	const layout = $derived(layoutSlot(discovery.items, 'form.list.toolbar'));
	const context = $derived(
		opened
			? hostContext(opened.item, {
					projectName: slot.name,
					locale: $locale ?? 'zh',
					theme
				})
			: undefined
	);
	const transport = $derived.by(() => {
		const frame = opened;
		if (!frame) return undefined;
		const item = frame.item;
		return createExtensionTransport({
			projectId,
			item,
			// Routes come from the current project discovery, never the manifest.
			enabledRoutes: () => enabledRoutes(projectDiscovery(projectId).snapshot().items, item.key),
			navigate: async (target) => {
				// A frame that is no longer shown does not navigate anymore.
				if (slot.frame?.token !== frame.token) return;
				const from = safeFrom(page.url.pathname + page.url.search, workspaceId, projectId);
				await goto(navigationHref(target, { workspaceId, projectId, key: item.key, from }));
			},
			onStale: () => void formStore()?.refresh()
		});
	});
	const deps: SlotDeps<PluginContributionItem> = {
		itemOf: (item) => item,
		loadEntry: (item) => loadEntry(projectId, item, { formId: formId ?? undefined }),
		projectName: () => projectName(projectId),
		refresh: () => void formStore()?.refresh()
	};

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
			const watched = slot.frame?.item ?? slot.active;
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
		closeSlot(slot, deps, message);
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
		const token = await openSlot(slot, item, deps);
		if (!token) return;
		await tick();
		if (slot.frame?.token === token) closeButton?.focus();
	}

	function onClose(reason: string) {
		closeDrawer(reason);
		if (reason !== '') void formStore()?.refresh();
	}

	/**
	 * The host of `token` retired its frame while the page was being left, and the page is live
	 * again because the navigation came back before the removal finished. The drawer the user had
	 * opened reopens with a fresh entry, as long as the contribution is still the same one.
	 */
	function retiredFor(token: number) {
		return () => void retireSlot(slot, token, discovery, deps);
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
				class="inline-flex h-8 items-center rounded-control border border-input px-2.5 text-xs font-medium text-fg hover:bg-surface"
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
					class="inline-flex h-8 cursor-pointer items-center rounded-control border border-input px-2.5 text-xs font-medium"
					>{$t('plugins.host.more')}</summary
				>
				<ul
					class="absolute right-0 z-20 mt-1 min-w-44 space-y-1 rounded-float border border-overlay-line bg-overlay p-2 text-sm shadow-overlay"
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
			<span class="text-xs text-fg-muted" role="status">{$t(notice)}</span>
		{/if}
	</div>
{/if}

{#if active}
	<div
		id={drawerId}
		role="dialog"
		tabindex="-1"
		aria-label={$t('plugins.host.openDrawer', { values: { label: active.contribution.label } })}
		class="fixed inset-y-0 right-0 z-40 flex w-full max-w-xl flex-col border-l border-overlay-line bg-overlay text-fg shadow-overlay"
		onkeydown={onKeydown}
	>
		<div
			class="flex items-center justify-between gap-2 border-b border-line p-3"
		>
			<h3 class="text-sm font-semibold">{active.name} · {active.contribution.label}</h3>
			<button
				type="button"
				bind:this={closeButton}
				onclick={() => closeDrawer()}
				class="rounded-control px-2 py-1 text-sm underline"
				>{$t('plugins.host.closeDrawer')}</button
			>
		</div>
		<div class="min-h-0 flex-1 overflow-auto">
			{#if phase === 'loading'}
				<p class="p-3 text-sm text-fg-muted" role="status">
					{$t('plugins.host.loading')}
				</p>
			{/if}
			{#if opened && transport && context}
				<!-- One frame per open: a later open never reuses a host that is still being removed. -->
				{#key opened.token}
					<PluginFrameHost
						html={opened.html}
						webSha256={opened.item.web_sha256}
						grants={[...opened.item.grants]}
						{context}
						{transport}
						title={$t('plugins.host.frameTitle', {
							values: { name: opened.item.name, label: opened.item.contribution.label }
						})}
						minHeight={frameHeights.panel.min}
						maxHeight={frameHeights.panel.max}
						{onClose}
						onRetired={retiredFor(opened.token)}
					/>
				{/key}
			{/if}
		</div>
	</div>
{/if}
