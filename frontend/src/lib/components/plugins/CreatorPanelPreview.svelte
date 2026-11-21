<script lang="ts">
	import { untrack } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import Button from '#lib/components/Button.svelte';
	import PluginFrameHost from '#lib/components/plugins/PluginFrameHost.svelte';
	import type { V3PreviewSpec } from '#lib/plugins/creator/registry.js';
	import type { PluginHostContext } from '#lib/plugins/bridge.js';
	import { createMockTransport, sampleRecordId } from '#lib/plugins/creator/ui-view-template.js';
	import {
		closePreview,
		initialPreviewFrame,
		retirePreview,
		retryPreview,
		type PreviewFrame
	} from '#lib/plugins/creator/preview-frame.js';

	let { html, v3 }: { html: string; v3: V3PreviewSpec } = $props();
	// The v3 preview is fixed when it opens; the caller remounts it for a new draft revision.
	const spec = untrack(() => v3);
	// Which frame is shown (creator/preview-frame.ts): every open mounts its own host.
	const shown = $state<PreviewFrame>(initialPreviewFrame());
	const picked = spec.manifest.contributions.find((item) => item.id === spec.contributionId);
	const grants = [
		'ui.display',
		...spec.manifest.reads.map((read) => `read:${read.id}`),
		...(spec.onAmountPreview && spec.manifest.capabilities.some((c) => c.name === 'amount.preview')
			? ['amount.preview']
			: [])
	];
	const transports = new Map(
		spec.manifest.reads.map((read) => [
			read.id,
			createMockTransport(read, spec.schema, spec.personal)
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
		spec.onViewed();
		if (method === 'amount.preview' && spec.onAmountPreview) {
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
</script>

{#if picked && previewContext}
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
{/if}
