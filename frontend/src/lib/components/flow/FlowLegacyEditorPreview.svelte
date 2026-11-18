<script lang="ts">
	import { t } from 'svelte-i18n';
	import FlowBodyAsset from './FlowBodyAsset.svelte';
	import { privateBodyAssetProvider } from '#lib/flow/body/providers.js';
	import FlowBodyChild from './FlowBodyChild.svelte';
	import FlowInlineText from './FlowInlineText.svelte';
	import type { FlowObjectView } from '#lib/api/flow.js';
	import type { FlowError } from '#lib/flow/types.js';
	import { legacyProseBlocks, canEnableLegacyEditor } from '#lib/flow/legacy-editor.js';
	import { flowErrorI18nKey } from '#lib/flow/errors.js';
	let {
		object,
		busy,
		error,
		onEnable,
		onRefresh
	}: {
		object: FlowObjectView;
		busy: boolean;
		error: FlowError | null;
		onEnable: () => Promise<void>;
		onRefresh: () => Promise<void>;
	} = $props();
	const bodyProvider = $derived(privateBodyAssetProvider(object.id));
	const blocks = $derived(legacyProseBlocks(object.semantic_content));
</script>

<section
	class="min-h-0 min-w-0 flex-1 overflow-y-auto bg-white px-5 py-6 sm:px-8 dark:bg-slate-950"
	data-testid="flow-legacy-editor-preview"
	data-flow-scroll-owner
	aria-busy={busy}
>
	<h1 class="break-words text-2xl font-semibold text-slate-900 dark:text-slate-100">
		{object.title || $t('flow.nav.untitled')}
	</h1>
	<div
		class="my-5 rounded-lg border border-slate-200 bg-slate-50 p-4 text-sm dark:border-slate-800 dark:bg-slate-900"
		role="status"
	>
		<p class="font-medium">{$t('flow.legacyEditor.title')}</p>
		<p class="mt-1 text-slate-600 dark:text-slate-300">
			{$t(
				object.editor_state === 'unsupported'
					? 'flow.legacyEditor.unsupported'
					: canEnableLegacyEditor(object)
						? 'flow.legacyEditor.description'
						: 'flow.legacyEditor.readOnly'
			)}
		</p>
		{#if canEnableLegacyEditor(object)}
			<button
				type="button"
				class="mt-3 rounded-md bg-slate-900 px-3 py-2 font-medium text-white disabled:opacity-50 dark:bg-slate-100 dark:text-slate-900"
				onclick={onEnable}
				disabled={busy}
				>{$t(busy ? 'flow.legacyEditor.enabling' : 'flow.legacyEditor.enable')}</button
			>
		{/if}
		<button
			type="button"
			class="ml-3 mt-3 rounded-md border border-slate-300 px-3 py-2 disabled:opacity-50 dark:border-slate-700"
			onclick={onRefresh}
			disabled={busy}>{$t('flow.legacyEditor.refresh')}</button
		>
	</div>
	{#if error}<p class="mb-4 text-sm text-red-700 dark:text-red-300" role="alert">
			{$t(flowErrorI18nKey(error))}
		</p>{/if}
	<div class="space-y-4 text-slate-900 dark:text-slate-100">
		{#each blocks as block (block.id)}
			<div style={`margin-left:${block.indent * 1.5}rem`}>
				{#if block.kind === 'image' || block.kind === 'attachment'}<FlowBodyAsset
						kind={block.kind}
						provider={bodyProvider}
						assetId={block.attrs.asset_id}
						alt={block.attrs.alt ?? ''}
						caption={block.attrs.caption ?? ''}
						name={block.attrs.name ?? ''}
					/>
				{:else if block.kind === 'childPage'}<FlowBodyChild childId={block.attrs.child_page_id} />
				{:else}<p
						class="whitespace-pre-wrap break-words"
						class:font-semibold={block.kind === 'heading'}
						class:font-mono={block.kind === 'codeBlock'}
					>
						<FlowInlineText text={block.text} spans={block.inline_spans} />
					</p>{/if}
			</div>
		{:else}<p class="text-sm text-slate-500">{$t('flow.legacyEditor.empty')}</p>{/each}
	</div>
</section>
