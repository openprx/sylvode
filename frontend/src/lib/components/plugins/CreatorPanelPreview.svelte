<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/components/Button.svelte';
	import PluginFrameHost from '#lib/components/plugins/PluginFrameHost.svelte';
	import type { PluginSlot, PluginSlotV3 } from '#lib/api/plugin-packages.js';
	import type { V3PreviewSpec } from '#lib/plugins/creator/registry.js';
	import {
		bridgeProtocol,
		isBridgeSignal,
		pluginRequest,
		type PluginHostContext
	} from '#lib/plugins/bridge.js';
	import { createMockTransport, sampleRecordId } from '#lib/plugins/creator/ui-view-template.js';
	import { creatorPlacement, creatorSlot } from '#lib/plugins/creator/template.js';
	import {
		closePreview,
		initialPreviewFrame,
		retirePreview,
		retryPreview,
		type PreviewFrame
	} from '#lib/plugins/creator/preview-frame.js';

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

	// The v3 preview is fixed when it opens; the caller remounts it for a new draft revision.
	const spec = untrack(() => v3);
	// Which frame is shown (creator/preview-frame.ts): every open mounts its own host.
	const shown = $state<PreviewFrame>(initialPreviewFrame());
	const picked = spec?.manifest.contributions.find((item) => item.id === spec.contributionId);
	const grants = spec
		? [
				'ui.display',
				...spec.manifest.reads.map((read) => `read:${read.id}`),
				...(spec.onAmountPreview &&
				spec.manifest.capabilities.some((c) => c.name === 'amount.preview')
					? ['amount.preview']
					: [])
			]
		: [];
	const transports = new Map(
		(spec?.manifest.reads ?? []).map((read) => [
			read.id,
			createMockTransport(read, spec?.schema ?? null, spec?.personal ?? true)
		])
	);
	const previewContext: PluginHostContext | null =
		spec && picked
			? {
					package: { key: spec.manifest.key, version: spec.manifest.version },
					contribution: {
						id: picked.id,
						kind: picked.kind,
						slot: picked.slot,
						view: picked.view
					},
					project: { name: spec.manifest.name },
					form: picked.form_key ? { key: picked.form_key } : null,
					record:
						picked.slot === 'form.record.detail' || picked.slot === 'form.record.actions'
							? { id: sampleRecordId }
							: null,
					route: picked.slot === 'project.view' ? { state: null } : null,
					locale: untrack(() => $locale)?.startsWith('zh') ? 'zh' : 'en',
					theme:
						typeof window !== 'undefined' &&
						window.matchMedia?.('(prefers-color-scheme: dark)').matches
							? 'dark'
							: 'light'
				}
			: null;

	/** The host of `attempt` retired while the page was being left; mount a fresh one. */
	function retiredFor(attempt: number) {
		return () => void retirePreview(shown, attempt);
	}

	async function previewTransport(
		method: string,
		params: Record<string, unknown>,
		requestId: string
	) {
		spec?.onViewed();
		if (method === 'amount.preview' && spec?.onAmountPreview) {
			const cents = await spec.onAmountPreview();
			return cents === null
				? ({ ok: false, code: 'failed' } as const)
				: ({ ok: true, result: { amount_cents: cents } } as const);
		}
		const transport = method.startsWith('data.') ? transports.get(String(params.read)) : undefined;
		return transport
			? transport(method, params, requestId)
			: ({ ok: false, code: 'method_unavailable' } as const);
	}

	onMount(() => {
		if (v3) return;
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
{#if spec && picked && previewContext}
	{#if shown.closed !== null}
		<div class="flex flex-wrap items-center gap-2 text-sm">
			{#if shown.closed}<p role="alert" class="text-danger">
					{$t(shown.closed)}
				</p>{/if}
			<Button variant="secondary" size="sm" onclick={() => retryPreview(shown)}
				>{$t('plugins.host.retry')}</Button
			>
		</div>
	{:else}
		{#key shown.attempt}
			<div class="rounded-control border border-line">
				<PluginFrameHost
					{html}
					webSha256={spec.webSha256}
					bridge={2}
					{grants}
					context={previewContext}
					transport={previewTransport}
					title={$t('plugins.host.frameTitle', {
						values: { name: spec.manifest.name, label: picked.label }
					})}
					minHeight={picked.kind === 'view' ? 320 : 160}
					maxHeight={picked.kind === 'view' ? 4000 : 800}
					onClose={(reason) => closePreview(shown, reason)}
					onRetired={retiredFor(shown.attempt)}
				/>
			</div>
		{/key}
	{/if}
{:else if failed}
	<p role="alert" class="text-sm text-danger">{$t('plugins.loadFailed')}</p>
{:else}
	{#if !ready}<p role="status" class="text-sm">{$t('plugins.loadingPanel')}</p>{/if}
	<iframe
		bind:this={frame}
		title={$t('plugins.creator.preview')}
		src="/plugin-host/frame.html"
		sandbox="allow-scripts"
		referrerpolicy="no-referrer"
		class="block h-72 w-full rounded-control border border-line"
		hidden={!ready}
	></iframe>
{/if}
