<script lang="ts">
	import { onDestroy } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { AssetBytesReader } from '#lib/flow/body/providers.js';
	import { bodyBytesValid, bodyDownloadName } from '#lib/flow/body/bytes.js';
	let {
		kind,
		assetId,
		provider,
		alt = '',
		caption = '',
		name = '',
		onReplace,
		onDetails,
		onContinue
	}: {
		kind: 'image' | 'attachment';
		assetId: string;
		provider: AssetBytesReader;
		alt?: string;
		caption?: string;
		name?: string;
		onReplace?: () => void;
		onDetails?: () => void;
		onContinue?: () => void;
	} = $props();
	let url = $state<string | null>(null),
		status = $state<'loading' | 'ready' | 'error'>('loading'),
		downloadName = $state('attachment'),
		retry = $state(0);
	let downloadAbort: AbortController | null = null;
	let destroyed = false;
	$effect(() => {
		void retry;
		const request = new AbortController();
		const id = assetId,
			reader = provider;
		let active = true;
		url = null;
		status = 'loading';
		if (kind === 'attachment') {
			status = 'ready';
			return () => {
				active = false;
				downloadAbort?.abort();
			};
		}
		const read = reader?.read(id, request.signal);
		if (!read) {
			status = 'error';
			return;
		}
		void read.then((result) => {
			if (!active) return;
			if (
				result.code !== 0 ||
				!result.data ||
				!bodyBytesValid('image', result.data.blob, result.data.headers)
			) {
				status = 'error';
				return;
			}
			url = URL.createObjectURL(result.data.blob);
			status = 'ready';
		});
		return () => {
			active = false;
			request.abort();
			if (url) {
				URL.revokeObjectURL(url);
				url = null;
			}
		};
	});
	onDestroy(() => {
		destroyed = true;
		downloadAbort?.abort();
		if (url) URL.revokeObjectURL(url);
	});
	async function download() {
		if (status === 'loading') return;
		downloadAbort?.abort();
		const request = new AbortController();
		downloadAbort = request;
		const requestAsset = assetId;
		const reader = provider;
		status = 'loading';
		const result = await reader.read(assetId, request.signal);
		if (
			destroyed ||
			request.signal.aborted ||
			requestAsset !== assetId ||
			reader.key !== provider.key
		)
			return;
		if (
			!result ||
			result.code !== 0 ||
			!result.data ||
			!bodyBytesValid('attachment', result.data.blob, result.data.headers)
		) {
			status = 'error';
			return;
		}
		downloadName = bodyDownloadName(result.data.headers);
		const blobUrl = URL.createObjectURL(result.data.blob);
		const a = document.createElement('a');
		a.href = blobUrl;
		a.download = downloadName;
		a.click();
		setTimeout(() => URL.revokeObjectURL(blobUrl), 1000);
		status = 'ready';
	}
</script>

<figure
	class="my-3 rounded-card border border-line p-3"
	data-testid="flow-body-asset"
	data-asset-kind={kind}
	data-asset-id={assetId}
	data-state={status}
>
	{#if kind === 'image'}
		{#if status === 'loading'}<p role="status" class="text-sm text-fg-muted">
				{$t('flow.body.loadingImage')}
			</p>
		{:else if status === 'error'}<div role="status" class="text-sm text-fg-muted">
				<span>{$t('flow.body.unavailableImage')}</span><button
					type="button"
					class="ml-2 underline"
					onclick={() => retry++}>{$t('flow.body.retry')}</button
				>
			</div>
		{:else if url}<img
				src={url}
				{alt}
				draggable="false"
				onerror={(event) => {
					const current = url;
					if (current && (event.currentTarget as HTMLImageElement).src === current) {
						URL.revokeObjectURL(current);
						url = null;
						status = 'error';
					}
				}}
				class="max-h-[70vh] max-w-full rounded object-contain"
			/>{/if}
	{:else}<button
			type="button"
			class="flex max-w-full items-center gap-2 text-left text-sm underline"
			disabled={status === 'loading'}
			onclick={download}
			data-testid="flow-body-file-download">📎 {name || $t('flow.body.file')}</button
		>{#if status === 'error'}<p role="status" class="text-sm text-danger">
				{$t('flow.body.downloadFailed')}
			</p>{/if}{/if}
	{#if caption}<figcaption class="mt-2 whitespace-pre-wrap text-sm text-fg-muted">
			{caption}
		</figcaption>{/if}
	{#if onReplace || onDetails || onContinue}<div
			class="mt-2 flex flex-wrap gap-3 text-xs text-fg-muted"
		>
			{#if onReplace}<button type="button" onclick={onReplace}>{$t('flow.body.replace')}</button
				>{/if}
			{#if onDetails}<button type="button" onclick={onDetails}>{$t('flow.body.editDetails')}</button
				>{/if}
			{#if onContinue}<button type="button" onclick={onContinue}
					>{$t('flow.body.continueWriting')}</button
				>{/if}
		</div>{/if}
</figure>
