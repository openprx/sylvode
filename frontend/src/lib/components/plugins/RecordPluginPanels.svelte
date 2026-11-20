<script lang="ts">
	import { onMount, tick, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/components/Button.svelte';
	import PluginFrameHost from './PluginFrameHost.svelte';
	import PluginPreferenceMenu from './PluginPreferenceMenu.svelte';
	import RecordPluginEntryButton from './RecordPluginEntryButton.svelte';
	import {
		contributionEntries,
		entryPluginKey,
		isContributionEntry,
		mergeEntries,
		panelEntries,
		sameEntry,
		type RecordActionSurfaceEntry,
		type RecordEntry,
		type RecordPluginActionSurface
	} from '#lib/plugins/placements.js';
	import { apiClient } from '#lib/api/client.js';
	import { pluginPackagesApi, type PluginPanel } from '#lib/api/plugin-packages.js';
	import type { PluginContributionItem } from '#lib/api/plugin-packages.js';
	import { samePanel } from '#lib/plugins/bridge.js';
	import {
		currentTheme,
		enabledRoutes,
		frameHeights,
		frameVerdict,
		loadEntry,
		navigationHref,
		partitionInline,
		projectDiscovery,
		projectName,
		recordDiscovery,
		safeFrom,
		slotLimits,
		verdictReasons,
		type DiscoveryState
	} from '#lib/plugins/contributions.js';
	import {
		closeSlot,
		createApplyConfirmation,
		initialSlot,
		openSlot,
		recordPreview,
		retireSlot,
		type SlotDeps,
		type SlotState
	} from '#lib/plugins/frame-slot.js';
	import {
		createExtensionTransport,
		createLegacyTransport,
		hostContext,
		type HostTransport
	} from '#lib/plugins/host-methods.js';
	import type { PluginHostContext } from '#lib/plugins/bridge.js';

	let {
		recordId,
		surface,
		onApplied
	}: {
		recordId: string;
		surface: RecordPluginActionSurface;
		onApplied: () => void;
	} = $props();
	const componentId = $props.id();
	const workspaceId = $derived(page.params.workspaceId ?? '');
	const projectId = $derived(page.params.projectId ?? '');

	/** Everything the host needs for the shown frame, derived from the slot's frame. */
	interface HostedFrame {
		token: number;
		bridge: 1 | 2;
		html: string;
		webSha256: string;
		grants: string[];
		title: string;
		heading: string;
		minHeight: number;
		maxHeight: number;
		placement?: { id: string; slot: string };
		item?: PluginContributionItem;
		panel?: PluginPanel;
		transport: HostTransport;
	}

	let panels = $state<PluginPanel[]>([]);
	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });
	// The open/retire state machine (frame-slot.ts) mutates this object; it is a deep $state proxy.
	const slot = $state<SlotState<RecordEntry>>(initialSlot());
	let theme = $state<'light' | 'dark'>(currentTheme());
	let heading: HTMLHeadingElement | undefined = $state();
	let closeButton: HTMLButtonElement | undefined = $state();
	let launch: HTMLElement | null = null;
	let confirmation: HTMLDialogElement | undefined = $state();
	const confirming = createApplyConfirmation({
		show: () => confirmation?.showModal(),
		hide: () => confirmation?.close()
	});
	let discoveryRunning = false;
	let lastDiscovery = 0;
	let disposed = false;
	let authSignal: AbortSignal | undefined;

	const active = $derived(slot.active);
	const notice = $derived(slot.notice);
	const preview = $derived(slot.preview);
	const deps: SlotDeps<RecordEntry> = {
		itemOf: (entry) => (isContributionEntry(entry) ? entry.item : null),
		loadEntry: (item) => loadEntry(projectId, item, { recordId }),
		projectName: () => projectName(projectId),
		refresh: () => refreshAll(),
		dismissConfirmation: () => confirming.dismiss(),
		alive: () => !disposed && !authSignal?.aborted
	};
	const frame = $derived.by((): HostedFrame | null => {
		const shown = slot.frame;
		if (!shown) return null;
		const token = shown.token;
		const entry = shown.entry;
		if (!isContributionEntry(entry)) {
			const { panel } = entry;
			return {
				token,
				bridge: 1,
				html: panel.html,
				webSha256: panel.web_sha256,
				grants: [...panel.grants],
				title: panel.name,
				heading: `${panel.name} · v${panel.version}`,
				// Protocol 1/2 frames keep their previous fixed height.
				minHeight: 288,
				maxHeight: frameHeights.panel.max,
				panel,
				...(panel.contributions ? { placement: { id: entry.id, slot: entry.slot } } : {}),
				transport: tracked(
					token,
					createLegacyTransport({ recordId, panel, onApplied: () => onApplied() })
				)
			};
		}
		const item = shown.item ?? entry.item;
		return {
			token,
			bridge: 2,
			html: shown.html,
			webSha256: item.web_sha256,
			grants: [...item.grants],
			title: $t('plugins.host.frameTitle', {
				values: { name: item.name, label: item.contribution.label }
			}),
			heading: `${item.name} · v${item.version}`,
			minHeight: frameHeights.panel.min,
			maxHeight: frameHeights.panel.max,
			item,
			transport: tracked(
				token,
				createExtensionTransport({
					projectId,
					item,
					recordId,
					// Routes come from the current project discovery, never the manifest.
					enabledRoutes: () =>
						enabledRoutes(projectDiscovery(projectId).snapshot().items, item.key),
					navigate: async (target) => {
						// A frame that is no longer shown does not navigate anymore.
						if (slot.frame?.token !== token) return;
						const from = safeFrom(page.url.pathname + page.url.search, workspaceId, projectId);
						await goto(navigationHref(target, { workspaceId, projectId, key: item.key, from }));
					},
					onStale: () => void recordDiscovery(projectId, recordId).refresh(),
					onApplied: () => onApplied()
				})
			)
		};
	});

	const v3Items = $derived(discovery.items);
	const detailParts = $derived(
		split(
			mergeEntries(
				panelEntries(panels, 'form.record.detail'),
				contributionEntries(v3Items, 'form.record.detail')
			),
			slotLimits['form.record.detail']
		)
	);
	const actionParts = $derived(
		split(
			mergeEntries(
				panelEntries(panels, 'form.record.actions'),
				contributionEntries(v3Items, 'form.record.actions')
			),
			slotLimits['form.record.actions']
		)
	);
	const details = $derived([...detailParts.inline, ...detailParts.more]);
	const context = $derived<PluginHostContext | undefined>(
		frame?.item
			? hostContext(frame.item, {
					projectName: slot.name,
					recordId,
					locale: $locale ?? 'zh',
					theme
				})
			: undefined
	);

	function split(entries: RecordEntry[], limit: number) {
		return partitionInline(
			entries,
			limit,
			(entry) => isContributionEntry(entry) && entry.item.preference.collapsed
		);
	}

	$effect(() => {
		surface.set({
			recordId,
			panelId: componentId + '-panel',
			entries: actionParts.inline,
			more: actionParts.more,
			active,
			toggle
		} satisfies RecordActionSurfaceEntry);
	});

	// Protocol 3 contributions for this record, shared and polled by the discovery store.
	$effect(() => {
		const id = projectId;
		const record = recordId;
		discovery = { status: 'loading', items: [] };
		if (!id) return;
		const store = recordDiscovery(id, record);
		const stop = store.subscribe((value) => (discovery = value));
		const release = store.retain();
		return () => {
			stop();
			release();
		};
	});

	$effect(() => {
		if (!frame?.item || !projectId) return;
		const release = [
			recordDiscovery(projectId, recordId).holdBusy(),
			projectDiscovery(projectId).retain()
		];
		return () => release.forEach((fn) => fn());
	});

	$effect(() => {
		const current = discovery;
		untrack(() => {
			const watched =
				slot.frame?.item ??
				(slot.active && isContributionEntry(slot.active) ? slot.active.item : null);
			if (!watched) return;
			const verdict = frameVerdict(current, watched);
			if (verdict !== 'ok') close(verdictReasons[verdict]);
		});
	});

	onMount(() => {
		const epoch = apiClient.getAuthSessionEpoch();
		authSignal = apiClient.authSessionSignal(epoch);
		authSignal.addEventListener('abort', sessionChanged, { once: true });
		const observer = new MutationObserver(() => (theme = currentTheme()));
		observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });
		void discover();
		const timer = setInterval(() => {
			if (slot.frame || Date.now() - lastDiscovery >= 10000) void discover();
		}, 2000);
		return () => {
			disposed = true;
			surface.update((value) => (value?.panelId === componentId + '-panel' ? null : value));
			authSignal?.removeEventListener('abort', sessionChanged);
			observer.disconnect();
			clearInterval(timer);
			confirming.settle(false);
		};
	});

	function sessionChanged() {
		panels = [];
		close('plugins.sessionChanged');
	}

	async function discover() {
		if (discoveryRunning || disposed || authSignal?.aborted) return;
		discoveryRunning = true;
		lastDiscovery = Date.now();
		try {
			const result = await pluginPackagesApi.discover(recordId);
			if (disposed || authSignal?.aborted) return;
			const legacy = frame?.panel;
			if (result.code !== 0 || !Array.isArray(result.data)) {
				panels = [];
				if (legacy)
					close(
						result.code === 401 || result.code === 403
							? 'plugins.accessChanged'
							: 'plugins.checkFailed'
					);
				return;
			}
			panels = result.data;
			if (legacy && !panels.some((panel) => samePanel(panel, legacy))) {
				close(
					panels.some((panel) => panel.key === legacy.key)
						? 'plugins.versionChanged'
						: 'plugins.accessChanged'
				);
			}
		} finally {
			discoveryRunning = false;
		}
	}

	function refreshAll() {
		void discover();
		if (projectId) void recordDiscovery(projectId, recordId).refresh();
	}

	function close(message = '', restoreFocus = true) {
		closeSlot(slot, deps, message);
		const trigger = launch;
		launch = null;
		if (restoreFocus)
			void tick().then(() => {
				trigger?.focus();
				if (!trigger?.isConnected || document.activeElement !== trigger) heading?.focus();
			});
	}

	function toggle(entry: RecordEntry, trigger: HTMLElement) {
		if (sameEntry(active, entry)) close();
		else void open(entry, trigger);
	}

	/** Records the previewed amount so the apply confirmation can show it. */
	function tracked(token: number, base: HostTransport): HostTransport {
		return async (method, params, requestId) => {
			const outcome = await base(method, params, requestId);
			if (method === 'amount.preview') {
				const result = outcome.ok ? (outcome.result as { amount_cents?: unknown }) : null;
				recordPreview(
					slot,
					token,
					typeof result?.amount_cents === 'number' ? result.amount_cents : null
				);
			}
			return outcome;
		};
	}

	async function open(entry: RecordEntry, trigger: HTMLElement) {
		close('', false);
		launch = trigger;
		const token = await openSlot(slot, entry, deps);
		if (!token) return;
		await tick();
		if (slot.frame?.token === token) closeButton?.focus();
	}

	function confirmSave() {
		if (authSignal?.aborted) return;
		confirming.confirm();
	}

	function onClose(reason: string) {
		close(reason);
		// The host closes an `amount.*` call that outlived its timeout. A timed-out write may have
		// committed, so reload the record.
		if (reason === 'plugins.actionTimedOut') onApplied();
		if (reason !== '') refreshAll();
	}

	/**
	 * The host of `token` retired its frame while the page was being left, and the page is live
	 * again because the navigation came back before the removal finished. A protocol 3 panel the
	 * user had open reopens with a fresh entry, if its contribution is still the same one; a
	 * pending apply confirmation is dismissed and the panel is idle again. Legacy panels close
	 * without a grace period and never get here.
	 */
	function retiredFor(token: number) {
		return () => void retireSlot(slot, token, discovery, deps);
	}
</script>

<svelte:window onfocus={() => void discover()} />

{#if details.length || notice || slot.frame || active || v3Items.length}
	<section
		class="space-y-2 border-b border-slate-200 p-4 dark:border-slate-700"
		aria-labelledby={componentId}
	>
		<div class="flex flex-wrap items-center justify-between gap-2">
			<h3 id={componentId} bind:this={heading} tabindex="-1" class="text-sm font-semibold">
				{$t('plugins.recordPlugins')}
			</h3>
			{#if projectId}<PluginPreferenceMenu {projectId} items={v3Items} />{/if}
		</div>
		{#if notice}
			<p class="text-sm text-slate-600 dark:text-slate-300" role="status">{$t(notice)}</p>
		{/if}
		{#if details.length}
			<ul class="flex flex-wrap gap-2" data-plugin-slot="form.record.detail">
				{#each detailParts.inline as entry (`${entryPluginKey(entry)}:${entry.id}`)}
					<li>
						<RecordPluginEntryButton
							{entry}
							panelId={componentId + '-panel'}
							expanded={sameEntry(active, entry)}
							onToggle={toggle}
							showVersion
						/>
					</li>
				{/each}
				{#if detailParts.more.length}
					<li>
						<details class="relative">
							<summary
								class="cursor-pointer rounded-md border border-slate-300 px-3 py-1.5 text-sm dark:border-slate-600"
								>{$t('plugins.host.more')}</summary
							>
							<ul
								class="absolute left-0 z-20 mt-1 min-w-44 space-y-1 rounded-md border border-slate-300 bg-white p-2 shadow dark:border-slate-600 dark:bg-slate-900"
							>
								{#each detailParts.more as entry (`${entryPluginKey(entry)}:${entry.id}`)}
									<li>
										<RecordPluginEntryButton
											{entry}
											panelId={componentId + '-panel'}
											expanded={sameEntry(active, entry)}
											onToggle={(next, trigger) => {
												const menu = trigger.closest('details');
												if (menu) menu.open = false;
												toggle(next, trigger);
											}}
											showVersion
										/>
									</li>
								{/each}
							</ul>
						</details>
					</li>
				{/if}
			</ul>
		{/if}
		{#if active}
			<div
				id={componentId + '-panel'}
				class="overflow-hidden rounded-md border border-slate-300 dark:border-slate-600"
			>
				<div class="flex flex-wrap items-center justify-between gap-2 px-3 py-1">
					<h4 class="text-sm font-medium">
						{frame?.heading ?? (isContributionEntry(active) ? active.item.name : active.panel.name)}
					</h4>
					<button
						type="button"
						bind:this={closeButton}
						onclick={() => close()}
						class="rounded-md px-2 py-1 text-sm underline focus-visible:outline-blue-600"
						>{$t('plugins.closePanel')}</button
					>
				</div>
				{#if frame}
					<!-- One frame per open: a later open never reuses a host that is still being removed. -->
					{#key frame.token}
						<PluginFrameHost
							html={frame.html}
							webSha256={frame.webSha256}
							bridge={frame.bridge}
							grants={frame.grants}
							{context}
							placement={frame.placement}
							transport={frame.transport}
							title={frame.title}
							minHeight={frame.minHeight}
							maxHeight={frame.maxHeight}
							{onClose}
							onRetired={frame.item ? retiredFor(frame.token) : undefined}
							onConfirmApply={() => confirming.request()}
						/>
					{/key}
				{:else}
					<p class="px-3 pb-2 text-sm text-slate-600 dark:text-slate-300" role="status">
						{$t('plugins.host.loading')}
					</p>
				{/if}
			</div>
		{/if}
	</section>
{/if}

<dialog
	bind:this={confirmation}
	aria-labelledby={`${componentId}-confirm`}
	class="max-w-md rounded-lg border border-slate-300 bg-white p-5 text-slate-900 shadow-xl backdrop:bg-slate-950/50 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
	onclose={() => confirming.settle(false)}
>
	<h3 id={`${componentId}-confirm`} class="text-base font-semibold">
		{$t('plugins.confirmAmount')}
	</h3>
	{#if preview !== null}
		<p class="mt-2 text-sm">
			{$t('plugins.confirmAmountBody', { values: { amount: (preview / 100).toFixed(2) } })}
		</p>
	{/if}
	<div class="mt-4 flex flex-wrap justify-end gap-2">
		<Button variant="secondary" onclick={() => confirmation?.close()}>{$t('common.cancel')}</Button>
		<Button onclick={confirmSave}>{$t('plugins.saveAmount')}</Button>
	</div>
</dialog>
