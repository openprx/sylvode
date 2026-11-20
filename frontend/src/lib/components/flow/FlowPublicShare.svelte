<script lang="ts">
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import {
		flowApi,
		publicAppearanceUrl,
		type FlowAppearance,
		type FlowPublicBlock
	} from '#lib/api/flow.js';
	import { pageVisibleBlocks } from '#lib/flow/inline-runs.js';
	import FlowInlineText from './FlowInlineText.svelte';
	import FlowBodyAsset from './FlowBodyAsset.svelte';
	import FlowBodyChild from './FlowBodyChild.svelte';
	import { publicBodyAssetProvider } from '#lib/flow/body/providers.js';
	import { publicBodyAtom } from '#lib/flow/body/presentation.js';
	import { galleryBackground } from '#lib/flow/appearance.js';

	let title = $state('');
	let blocks = $state<FlowPublicBlock[]>([]);
	let appearance = $state<FlowAppearance | null>(null);
	let status = $state<'loading' | 'ready' | 'missing'>('loading');
	let token = $state('');
	const bodyProvider = $derived(publicBodyAssetProvider(token));
	let collapsed = $state<ReadonlySet<number>>(new Set());
	const visibleBlocks = $derived(pageVisibleBlocks(blocks, collapsed));

	const cover = $derived(appearance?.cover ?? null);
	const gallery = $derived(cover?.kind === 'gallery' ? galleryBackground(cover.gallery_id) : null);
	const coverUrl = $derived(
		cover?.kind === 'upload' ? publicAppearanceUrl(token, cover.asset_id) : null
	);
	const iconUrl = $derived(
		appearance?.icon?.kind === 'image' ? publicAppearanceUrl(token, appearance.icon.asset_id) : null
	);
	const emoji = $derived(appearance?.icon?.kind === 'emoji' ? appearance.icon.emoji : null);
	const position = $derived(cover?.position ?? 0.5);

	$effect(() => {
		const next = page.params.token ?? '';
		token = next;
		status = 'loading';
		if (!next) {
			status = 'missing';
			return;
		}
		void flowApi.getPublicShare(next).then((result) => {
			if (page.params.token !== next) return;
			if (result.code === 0 && result.data) {
				title = result.data.title;
				blocks = result.data.blocks ?? [];
				collapsed = new Set(
					blocks.flatMap((block, index) =>
						block.kind === 'toggle' && block.collapsed ? [index] : []
					)
				);
				appearance = result.data.appearance ?? null;
				status = 'ready';
			} else {
				status = 'missing';
			}
		});
	});

	function indentStyle(indent: number): string {
		return `margin-left: ${Math.max(0, Math.min(indent, 7)) * 1.5}rem`;
	}
</script>

<main class="min-h-screen bg-white dark:bg-slate-950" data-testid="flow-public-page">
	{#if status === 'loading'}
		<p class="mx-auto max-w-3xl px-4 py-10 text-slate-500">{$t('flow.publicShare.loading')}</p>
	{:else if status === 'missing'}
		<p class="mx-auto max-w-3xl px-4 py-10 text-fg">
			{$t('flow.publicShare.missing')}
		</p>
	{:else}
		{#if cover}
			<div class="h-48 w-full overflow-hidden" data-testid="flow-public-cover">
				{#if gallery}
					<div
						class="h-[200%] w-full"
						style:background={gallery}
						style:transform={`translateY(-${position * 50}%)`}
					></div>
				{:else if coverUrl}
					<img
						src={coverUrl}
						alt=""
						class="h-[200%] w-full object-cover"
						style:object-position={`center ${position * 100}%`}
					/>
				{/if}
			</div>
		{/if}
		<article
			class="flow-canvas prose prose-slate mx-auto max-w-3xl px-16 pt-8 pb-16 dark:prose-invert"
		>
			{#if emoji}
				<p class="text-5xl">{emoji}</p>
			{:else if iconUrl}
				<img src={iconUrl} alt="" class="h-16 w-16 rounded object-cover" />
			{/if}
			<h1 class="mb-6 text-4xl font-semibold text-fg">
				{title || $t('flow.publicShare.untitled')}
			</h1>
			<p class="text-xs text-slate-500">{$t('flow.publicShare.readOnly')}</p>
			{#each visibleBlocks as { block, index, order } (`${block.kind}-${index}`)}
				{@const body = publicBodyAtom(block)}
				{#if block.kind === 'heading'}
					{#if block.level === 1}
						<h1 style={indentStyle(block.indent)}>
							<FlowInlineText text={block.text} spans={block.inline_spans} />
						</h1>
					{:else if block.level === 3}
						<h3 style={indentStyle(block.indent)}>
							<FlowInlineText text={block.text} spans={block.inline_spans} />
						</h3>
					{:else}
						<h2 style={indentStyle(block.indent)}>
							<FlowInlineText text={block.text} spans={block.inline_spans} />
						</h2>
					{/if}
				{:else if block.kind === 'bulletItem'}
					<div class="flow-bullet-item" style={indentStyle(block.indent)}>
						<FlowInlineText text={block.text} spans={block.inline_spans} />
					</div>
				{:else if block.kind === 'orderedItem'}
					<div style={indentStyle(block.indent)}>
						<span aria-hidden="true">{order}. </span><FlowInlineText
							text={block.text}
							spans={block.inline_spans}
						/>
					</div>
				{:else if block.kind === 'todoItem'}
					<div class:line-through={block.checked} style={indentStyle(block.indent)}>
						<input
							type="checkbox"
							checked={block.checked}
							disabled
							aria-label={block.text}
						/><FlowInlineText text={block.text} spans={block.inline_spans} />
					</div>
				{:else if block.kind === 'blockquote'}
					<blockquote style={indentStyle(block.indent)}>
						<FlowInlineText text={block.text} spans={block.inline_spans} />
					</blockquote>
				{:else if block.kind === 'toggle'}
					<div class="flex items-start gap-1" style={indentStyle(block.indent)}>
						<button
							type="button"
							class="px-1"
							aria-label={collapsed.has(index) ? 'Expand section' : 'Collapse section'}
							aria-expanded={!collapsed.has(index)}
							onclick={() => {
								const next = new Set(collapsed);
								if (next.has(index)) next.delete(index);
								else next.add(index);
								collapsed = next;
							}}>{collapsed.has(index) ? '▸' : '▾'}</button
						><span><FlowInlineText text={block.text} spans={block.inline_spans} /></span>
					</div>
				{:else if block.kind === 'divider'}<hr style={indentStyle(block.indent)} />
				{:else if block.kind === 'codeBlock'}
					<pre style={indentStyle(block.indent)}><code>{block.text}</code></pre>
				{:else if body?.kind === 'image' || body?.kind === 'attachment'}<div
						style={indentStyle(block.indent)}
					>
						<FlowBodyAsset
							kind={body.kind}
							assetId={body.assetId}
							provider={bodyProvider}
							alt={body.alt}
							caption={body.caption}
							name={body.name}
						/>
					</div>
				{:else if body?.kind === 'childPage'}<div style={indentStyle(block.indent)}>
						<FlowBodyChild childId="" publicView />
					</div>
				{:else if block.kind === 'embed'}
					<p
						class="rounded-md border border-line px-3 py-2 text-sm text-slate-500"
					>
						{$t('flow.publicShare.embed')}
					</p>
				{:else}
					<p style={indentStyle(block.indent)}>
						<FlowInlineText text={block.text} spans={block.inline_spans} />
					</p>
				{/if}
			{/each}
		</article>
	{/if}
</main>

<style>
	:global(.flow-canvas .flow-bullet-item) {
		position: relative;
		padding-left: 1.25rem;
	}
	:global(.flow-canvas .flow-bullet-item::before) {
		content: '•';
		position: absolute;
		left: 0.25rem;
	}
	:global(.flow-canvas pre) {
		background: var(--color-slate-100);
		border-radius: 0.375rem;
		padding: 0.75rem;
		overflow-x: auto;
	}
	:global(.dark .flow-canvas pre) {
		background: var(--color-slate-900);
	}
</style>
