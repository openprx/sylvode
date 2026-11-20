<script lang="ts">
	import { tick } from 'svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import { newIdempotencyKey } from '#lib/flow/package-export.js';
	import { flowApi, type PortablePagePackagePreview } from '#lib/api/flow.js';
	import { MAX_PORTABLE_PAGE_BYTES, portablePageFailure } from '#lib/flow/portable-page-package.js';

	let {
		objectId,
		canImport,
		exportReady,
		oncreated
	}: {
		objectId: string;
		canImport: boolean;
		exportReady: boolean;
		oncreated?: (objectId: string) => void;
	} = $props();

	let exported = $state.raw<{ archive: Blob; preview: PortablePagePackagePreview | null } | null>(
		null
	);
	let imported = $state.raw<{
		file: File;
		preview: PortablePagePackagePreview;
		key: string;
	} | null>(null);
	let pending = $state<'export' | 'read' | 'import' | null>(null);
	let exportError = $state<string | null>(null);
	let importError = $state<string | null>(null);
	let targetId = $state<string | null>(null);
	let reviewHeading = $state<HTMLElement | null>(null);
	let openPageLink = $state<HTMLAnchorElement | null>(null);
	const id = $props.id();
	const action =
		'min-h-11 rounded-control border border-fg bg-raised px-3 text-sm font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-control disabled:border-line';

	async function prepareExport() {
		if (pending || !exportReady) return;
		pending = 'export';
		exported = null;
		exportError = null;
		try {
			const result = await flowApi.exportPortablePage(objectId);
			if (result.code !== 0 || !result.data) {
				exportError = portablePageFailure(result);
				return;
			}
			const archive = result.data.blob;
			if (!result.data.headers.get('Content-Type')?.startsWith('application/zip')) {
				exportError = 'flow.portablePackage.failed';
				return;
			}
			const review = await flowApi.previewPortablePage(objectId, archive, 'export');
			exported = { archive, preview: review.code === 0 ? review.data : null };
		} catch {
			exportError = 'flow.portablePackage.failed';
		} finally {
			pending = null;
		}
	}

	function download() {
		if (!exported) return;
		const url = URL.createObjectURL(exported.archive);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${objectId}.sylvode-page.zip`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
	}

	async function chooseFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file || pending || !canImport) return;
		imported = null;
		importError = null;
		targetId = null;
		if (file.size > MAX_PORTABLE_PAGE_BYTES) {
			importError = 'flow.portablePackage.tooLarge';
			return;
		}
		pending = 'read';
		try {
			const result = await flowApi.previewPortablePage(objectId, file);
			if (result.code !== 0 || !result.data) {
				importError = portablePageFailure(result);
				return;
			}
			imported = { file, preview: result.data, key: newIdempotencyKey() };
			await tick();
			reviewHeading?.focus();
		} catch {
			importError = 'flow.portablePackage.invalid';
		} finally {
			pending = null;
		}
	}

	async function importPage() {
		if (!imported || pending || !canImport || targetId) return;
		pending = 'import';
		importError = null;
		try {
			const result = await flowApi.importPortablePage(
				objectId,
				imported.file,
				imported.preview.package_sha256,
				imported.key
			);
			if (result.code !== 0 || !result.data) {
				importError = portablePageFailure(result);
				return;
			}
			targetId = result.data.object_id;
			oncreated?.(targetId);
			await tick();
			openPageLink?.focus();
		} catch {
			importError = 'flow.portablePackage.failed';
		} finally {
			pending = null;
		}
	}
</script>

{#snippet summary(packageData: PortablePagePackagePreview)}
	<p class="break-words font-medium">{packageData.title}</p>
	<div class="space-y-2">
		<p class="text-xs text-fg-muted">
			{$t('flow.portablePackage.contents', {
				values: {
					blocks: String(packageData.block_count),
					media: String(packageData.asset_count),
					links: String(packageData.linked_page_count)
				}
			})}
		</p>
		{#if packageData.blocks.some((block) => block.text)}
			<details class="rounded border border-line p-2">
				<summary class="cursor-pointer">
					{$t('flow.portablePackage.body')}
				</summary>
				<div class="mt-2 max-h-56 space-y-1 overflow-auto">
					{#each packageData.blocks as block, index (index)}
						{#if block.text}<p class="whitespace-pre-wrap break-words">{block.text}</p>{/if}
					{/each}
				</div>
			</details>
		{/if}
		{#if packageData.assets.length}
			<h4 class="text-xs font-medium">{$t('flow.portablePackage.media')}</h4>
			<ul class="max-h-56 list-disc space-y-1 overflow-auto pl-5">
				{#each packageData.assets as media, index (index)}
					<li class="break-words">
						{media.filename}
						<span class="text-xs text-fg-muted">
							({$t('flow.package.bytes', { values: { n: media.size.toLocaleString() } })})
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
{/snippet}

<section
	aria-labelledby={`${id}-heading`}
	class="space-y-3 text-sm"
	data-testid="flow-portable-page-package"
>
	<h2 id={`${id}-heading`} class="text-sm font-semibold text-fg">
		{$t('flow.portablePackage.heading')}
	</h2>
	<p class="text-xs text-fg-muted">{$t('flow.portablePackage.scope')}</p>
	<button
		type="button"
		class={action}
		disabled={!!pending || !exportReady}
		aria-describedby={!exportReady ? `${id}-sync` : undefined}
		onclick={prepareExport}
		data-testid="flow-portable-prepare">{$t('flow.portablePackage.prepare')}</button
	>
	{#if !exportReady}
		<p id={`${id}-sync`} class="text-xs text-fg-muted">
			{$t('flow.portablePackage.waitForSync')}
		</p>
	{/if}
	{#if pending === 'export'}<p role="status">{$t('flow.portablePackage.preparing')}</p>{/if}
	{#if exportError}<p class="text-danger" role="alert">
			{$t(exportError)}
		</p>{/if}
	{#if exported}
		<div
			class="space-y-2 rounded-card border border-line p-3"
			data-testid="flow-portable-export-review"
		>
			<h3 class="font-medium">{$t('flow.portablePackage.exportReview')}</h3>
			{#if exported.preview}
				{@render summary(exported.preview)}
			{:else}
				<p role="status">{$t('flow.portablePackage.reviewUnavailable')}</p>
			{/if}
			<button type="button" class={action} onclick={download} data-testid="flow-portable-download">
				{$t('flow.portablePackage.download')}
			</button>
		</div>
	{/if}
	{#if canImport}
		<div class="space-y-2 border-t border-line pt-3">
			<label class="block space-y-1">
				<span class="block font-medium">{$t('flow.portablePackage.selectFile')}</span>
				<input
					type="file"
					accept=".sylvode-page.zip,application/zip"
					disabled={!!pending}
					onchange={chooseFile}
					data-testid="flow-portable-file"
					class="block w-full min-w-0 text-sm file:mr-2 file:min-h-11 file:rounded file:border-0 file:bg-surface file:px-2"
					aria-describedby={`${id}-destination`}
				/>
			</label>
			<p id={`${id}-destination`} class="text-xs text-fg-muted">
				{$t('flow.portablePackage.destination')}
			</p>
			{#if pending === 'read'}<p role="status">{$t('flow.portablePackage.reading')}</p>{/if}
			{#if imported}
				<div
					class="space-y-2 rounded-card border border-line p-3"
					data-testid="flow-portable-import-review"
				>
					<h3
						bind:this={reviewHeading}
						tabindex="-1"
						class="break-words font-medium"
					>
						{$t('flow.portablePackage.importReview', { values: { name: imported.file.name } })}
					</h3>
					{@render summary(imported.preview)}
					{#if !targetId}
						<button
							type="button"
							class={action}
							disabled={!!pending}
							onclick={importPage}
							data-testid="flow-portable-create"
						>
							{$t('flow.portablePackage.create')}
						</button>
					{/if}
				</div>
			{/if}
			{#if pending === 'import'}<p role="status">{$t('flow.portablePackage.creating')}</p>{/if}
			{#if importError}<p class="text-danger" role="alert">
					{$t(importError)}
				</p>{/if}
			{#if targetId}
				<p role="status">{$t('flow.portablePackage.created')}</p>
				<a
					bind:this={openPageLink}
					data-testid="flow-portable-open"
					class="inline-block py-2 text-fg underline"
					href={page.params.workspaceId
						? resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
								workspaceId: page.params.workspaceId,
								objectId: targetId
							})
						: resolve('/(app)/flow/[objectId]', { objectId: targetId })}
				>
					{$t('flow.portablePackage.open')}
				</a>
			{/if}
		</div>
	{:else}
		<p class="text-xs text-fg-muted">
			{$t('flow.portablePackage.editRequired')}
		</p>
	{/if}
</section>
