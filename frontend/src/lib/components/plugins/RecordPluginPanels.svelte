<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import Button from '#lib/components/Button.svelte';
	import RecordPluginEntryButton from './RecordPluginEntryButton.svelte';
	import {
		panelEntries,
		sameEntry,
		type RecordPluginActionSurface,
		type RecordPluginEntry
	} from '#lib/plugins/placements.js';
	import { apiClient } from '#lib/api/client.js';
	import { pluginPackagesApi, type PluginPanel } from '#lib/api/plugin-packages.js';
	import {
		bridgeProtocol,
		isBridgeSignal,
		pluginRequest,
		samePanel,
		validResult,
		verifyPanelHtml
	} from '#lib/plugins/bridge.js';

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
	let panels = $state<PluginPanel[]>([]);
	let active = $state.raw<RecordPluginEntry | null>(null);
	const current = $derived(active?.panel ?? null);
	const details = $derived(panelEntries(panels, 'form.record.detail'));
	const actions = $derived(panelEntries(panels, 'form.record.actions'));
	let notice = $state('');
	let stage = $state<'closed' | 'verifying' | 'opening' | 'ready'>('closed');
	let frame: HTMLIFrameElement | undefined = $state();
	let heading: HTMLHeadingElement | undefined = $state();
	let closeButton: HTMLButtonElement | undefined = $state();
	let launch: HTMLElement | null = null;
	let confirmation: HTMLDialogElement | undefined = $state();
	let confirmRequest = $state<string | null>(null);
	let preview = $state<{ requestId: string; cents: number } | null>(null);
	let port: MessagePort | undefined;
	let busy = false;
	let timeout: ReturnType<typeof setTimeout> | undefined;
	let discoveryRunning = false;
	let lastDiscovery = 0;
	let disposed = false;
	let openingRun = 0;
	let epoch = 0;
	let authSignal: AbortSignal | undefined;
	const requests = new Set<string>();

	$effect(() => {
		surface.set({ recordId, panelId: componentId + '-panel', entries: actions, active, toggle });
	});

	onMount(() => {
		epoch = apiClient.getAuthSessionEpoch();
		authSignal = apiClient.authSessionSignal(epoch);
		authSignal.addEventListener('abort', sessionChanged, { once: true });
		void discover();
		const timer = setInterval(() => {
			if (current || Date.now() - lastDiscovery >= 10000) void discover();
		}, 2000);
		return () => {
			disposed = true;
			surface.update((value) => (value?.panelId === componentId + '-panel' ? null : value));
			authSignal?.removeEventListener('abort', sessionChanged);
			clearInterval(timer);
			clearChannel();
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
			if (result.code !== 0 || !Array.isArray(result.data)) {
				panels = [];
				if (current)
					close(
						result.code === 401 || result.code === 403
							? 'plugins.accessChanged'
							: 'plugins.checkFailed'
					);
				return;
			}
			panels = result.data;
			if (current && !panels.some((panel) => samePanel(panel, current!))) {
				close(
					panels.some((panel) => panel.key === current?.key)
						? 'plugins.versionChanged'
						: 'plugins.accessChanged'
				);
			}
		} finally {
			discoveryRunning = false;
		}
	}

	function clearChannel() {
		clearTimeout(timeout);
		port?.close();
		port = undefined;
		preview = null;
		busy = false;
		requests.clear();
		confirmation?.close();
		confirmRequest = null;
	}

	function close(message = '', restoreFocus = true) {
		openingRun++;
		clearChannel();
		stage = 'closed';
		active = null;
		notice = message;
		const trigger = launch;
		launch = null;
		if (restoreFocus)
			void tick().then(() => {
				if (trigger?.isConnected) trigger.focus();
				else heading?.focus();
			});
	}

	function toggle(entry: RecordPluginEntry, trigger: HTMLElement) {
		if (sameEntry(active, entry)) close();
		else void open(entry, trigger);
	}

	async function open(entry: RecordPluginEntry, trigger: HTMLElement) {
		close('', false);
		launch = trigger;
		const mine = ++openingRun;
		active = entry;
		const { panel } = entry;
		stage = 'verifying';
		let verified = false;
		try {
			verified = await verifyPanelHtml(panel);
		} catch {
			/* Unavailable integrity verification fails closed. */
		}
		if (mine !== openingRun || disposed || authSignal?.aborted) return;
		if (!verified) {
			close('plugins.integrityFailed');
			return;
		}
		stage = 'opening';
		timeout = setTimeout(() => close('plugins.loadFailed'), 7000);
		await tick();
		if (mine === openingRun) closeButton?.focus();
	}

	function onWindowMessage(event: MessageEvent) {
		if (!current || !frame || event.source !== frame.contentWindow || event.origin !== 'null')
			return;
		if (isBridgeSignal(event.data, 'sylvode-plugin-shell-invalidated')) {
			close('plugins.loadFailed');
			return;
		}
		if (stage !== 'opening' || port || !isBridgeSignal(event.data, 'sylvode-plugin-shell-ready'))
			return;
		const channel = new MessageChannel();
		port = channel.port1;
		port.onmessage = ({ data }) => void onMessage(data);
		frame.contentWindow?.postMessage(
			{
				protocol: bridgeProtocol,
				type: 'sylvode-plugin-shell-init',
				html: current.html,
				grants: $state.snapshot(current.grants),
				...(active && current.contributions
					? { placement: { id: active.id, slot: active.slot } }
					: {})
			},
			'*',
			[channel.port2]
		);
	}

	function result(requestId: string, errorKey: string) {
		port?.postMessage({
			protocol: bridgeProtocol,
			type: 'result',
			request_id: requestId,
			ok: false,
			message: $t(errorKey)
		});
	}

	async function onMessage(data: unknown) {
		if (stage === 'opening' && isBridgeSignal(data, 'ready')) {
			clearTimeout(timeout);
			stage = 'ready';
			return;
		}
		if (stage !== 'ready' || !current) return;
		if (isBridgeSignal(data, 'close')) {
			close();
			return;
		}
		const parsed = pluginRequest(data);
		if (!parsed) return;
		const { method, requestId } = parsed;
		if (!method || !current.grants.includes(method)) {
			result(requestId, 'plugins.actionUnavailable');
			return;
		}
		if (busy || requests.has(requestId) || requests.size >= 100) {
			result(requestId, 'plugins.actionBusy');
			return;
		}
		requests.add(requestId);
		busy = true;
		const mine = openingRun;
		const panel = current;
		const authorizedPreview = preview;
		if (method === 'amount.apply') {
			if (!authorizedPreview) {
				result(requestId, 'plugins.previewRequired');
				busy = false;
				return;
			}
			confirmRequest = requestId;
			await tick();
			if (mine !== openingRun) return;
			confirmation?.showModal();
			return;
		}
		preview = null;
		await invoke(panel, method, requestId, mine);
	}

	function dismissConfirmation() {
		if (!confirmRequest) return;
		result(confirmRequest, 'plugins.actionCanceled');
		confirmRequest = null;
		busy = false;
	}

	function confirm() {
		if (!confirmRequest || !current || !preview || authSignal?.aborted) return;
		const requestId = confirmRequest;
		confirmRequest = null;
		confirmation?.close();
		void invoke(current, 'amount.apply', requestId, openingRun, preview.requestId);
	}

	async function invoke(
		panel: PluginPanel,
		method: 'amount.preview' | 'amount.apply',
		requestId: string,
		mine: number,
		previewRequestId?: string
	) {
		// Close the port on timeout. A timed-out write may have committed, so reload the record.
		timeout = setTimeout(() => {
			if (mine !== openingRun) return;
			if (method === 'amount.apply') onApplied();
			close('plugins.actionTimedOut');
		}, 10000);
		const response = await pluginPackagesApi.invoke(
			recordId,
			panel,
			method,
			requestId,
			previewRequestId
		);
		if (disposed || mine !== openingRun || !apiClient.isAuthSessionEpoch(epoch)) return;
		clearTimeout(timeout);
		busy = false;
		if (response.code !== 0 || !response.data || !validResult(response.data, panel, requestId)) {
			result(requestId, 'plugins.actionFailed');
			preview = null;
			if (response.code === 401 || response.code === 403 || response.code === 404) {
				close('plugins.accessChanged');
				void discover();
			}
			return;
		}
		if (method === 'amount.preview') preview = { requestId, cents: response.data.amount_cents };
		else {
			preview = null;
			onApplied();
		}
		port?.postMessage({
			protocol: bridgeProtocol,
			type: 'result',
			request_id: requestId,
			ok: true,
			result: { amount_cents: response.data.amount_cents }
		});
	}
</script>

<svelte:window onmessage={onWindowMessage} onfocus={() => void discover()} />

{#if details.length || notice || current}
	<section
		class="space-y-2 border-b border-slate-200 p-4 dark:border-slate-700"
		aria-labelledby={componentId}
	>
		<h3 id={componentId} bind:this={heading} tabindex="-1" class="text-sm font-semibold">
			{$t('plugins.recordPlugins')}
		</h3>
		{#if notice}
			<p class="text-sm text-slate-600 dark:text-slate-300" role="status">{$t(notice)}</p>
		{/if}
		{#if details.length}
			<ul class="flex flex-wrap gap-2" data-plugin-slot="form.record.detail">
				{#each details as entry (`${entry.panel.key}:${entry.id}`)}
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
			</ul>
		{/if}
		{#if current}
			<div
				id={componentId + '-panel'}
				class="overflow-hidden rounded-md border border-slate-300 dark:border-slate-600"
			>
				<div class="flex flex-wrap items-center justify-between gap-2 px-3 py-1">
					<h4 class="text-sm font-medium">{current.name} · v{current.version}</h4>
					<button
						type="button"
						bind:this={closeButton}
						onclick={() => close()}
						class="rounded-md px-2 py-1 text-sm underline focus-visible:outline-blue-600"
						>{$t('plugins.closePanel')}</button
					>
				</div>
				{#if stage === 'verifying' || stage === 'opening'}
					<p class="px-3 pb-2 text-sm text-slate-600 dark:text-slate-300" role="status">
						{$t('plugins.loadingPanel')}
					</p>
				{/if}
				{#if stage === 'opening' || stage === 'ready'}
					<iframe
						bind:this={frame}
						title={current.name}
						src="/plugin-host/frame.html"
						sandbox="allow-scripts"
						referrerpolicy="no-referrer"
						class="block h-72 w-full border-0"
						hidden={stage !== 'ready'}
					></iframe>
				{/if}
			</div>
		{/if}
	</section>
{/if}

<dialog
	bind:this={confirmation}
	aria-labelledby={`${componentId}-confirm`}
	class="max-w-md rounded-lg border border-slate-300 bg-white p-5 text-slate-900 shadow-xl backdrop:bg-slate-950/50 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100"
	onclose={dismissConfirmation}
>
	<h3 id={`${componentId}-confirm`} class="text-base font-semibold">
		{$t('plugins.confirmAmount')}
	</h3>
	{#if preview}
		<p class="mt-2 text-sm">
			{$t('plugins.confirmAmountBody', { values: { amount: (preview.cents / 100).toFixed(2) } })}
		</p>
	{/if}
	<div class="mt-4 flex flex-wrap justify-end gap-2">
		<Button variant="secondary" onclick={() => confirmation?.close()}>{$t('common.cancel')}</Button>
		<Button onclick={confirm}>{$t('plugins.saveAmount')}</Button>
	</div>
</dialog>
