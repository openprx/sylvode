<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { PluginSlot, PluginSlotV3 } from '#lib/api/plugin-packages.js';
	import type { V3PreviewSpec } from '#lib/plugins/creator/registry.js';
	import { bridgeProtocol, isBridgeSignal, pluginRequest } from '#lib/plugins/bridge.js';
	import { creatorPlacement, creatorSlot } from '#lib/plugins/creator/template.js';

	let {
		html,
		slot,
		placementId,
		onPreview,
		v3
	}: {
		html: string;
		slot: PluginSlot | PluginSlotV3;
		placementId?: string;
		onPreview?: () => number | null | Promise<number | null>;
		v3?: V3PreviewSpec;
	} = $props();
	let frame: HTMLIFrameElement | undefined = $state();
	let port: MessagePort | undefined;
	let ready = $state(false);
	let failed = $state(false);
	let timer: ReturnType<typeof setTimeout>;

	onMount(() => {
		timer = setTimeout(() => {
			failed = true;
			port?.close();
		}, 7000);
		return () => {
			clearTimeout(timer);
			port?.close();
		};
	});

	function receive(event: MessageEvent) {
		if (v3 || !frame || event.source !== frame.contentWindow || event.origin !== 'null' || failed)
			return;
		if (isBridgeSignal(event.data, 'sylvode-plugin-shell-invalidated')) {
			failed = true;
			port?.close();
			return;
		}
		if (port || !isBridgeSignal(event.data, 'sylvode-plugin-shell-ready')) return;
		const channel = new MessageChannel();
		port = channel.port1;
		port.onmessage = async ({ data }) => {
			if (isBridgeSignal(data, 'ready')) {
				clearTimeout(timer);
				ready = true;
				return;
			}
			const request = pluginRequest(data);
			if (!request || !ready || failed) return;
			const { requestId, method } = request;
			const cents = method === 'amount.preview' && onPreview ? await onPreview() : null;
			const response =
				cents === null
					? {
							ok: false,
							message: $t(
								method === 'amount.preview'
									? 'plugins.library.previewValues'
									: 'plugins.creator.previewReadOnly'
							)
						}
					: { ok: true, result: { amount_cents: cents } };
			port?.postMessage({
				protocol: bridgeProtocol,
				type: 'result',
				request_id: requestId,
				...response
			});
		};
		frame.contentWindow?.postMessage(
			{
				protocol: bridgeProtocol,
				type: 'sylvode-plugin-shell-init',
				html,
				grants: ['amount.preview'],
				placement: placementId ? { id: placementId, slot } : creatorPlacement(creatorSlot(slot))
			},
			'*',
			[channel.port2]
		);
	}
</script>

<svelte:window onmessage={receive} />
{#if v3}
	<p role="status" class="text-sm">{$t('plugins.creatorV3.previewPending')}</p>
{:else if failed}
	<p role="alert" class="text-sm text-red-800 dark:text-red-200">{$t('plugins.loadFailed')}</p>
{:else}
	{#if !ready}<p role="status" class="text-sm">{$t('plugins.loadingPanel')}</p>{/if}
	<iframe
		bind:this={frame}
		title={$t('plugins.creator.preview')}
		src="/plugin-host/frame.html"
		sandbox="allow-scripts"
		referrerpolicy="no-referrer"
		class="block h-72 w-full rounded border border-slate-300 dark:border-slate-600"
		hidden={!ready}
	></iframe>
{/if}
