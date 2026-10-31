<script lang="ts">
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import { flowApi, publicAppearanceUrl, type FlowAppearance, type FlowPublicBlock } from '#lib/api/flow.js';
	import { galleryBackground } from '#lib/flow/appearance.js';

	let title = $state('');
	let blocks = $state<FlowPublicBlock[]>([]);
	let appearance = $state<FlowAppearance | null>(null);
	let status = $state<'loading' | 'ready' | 'missing'>('loading');
	let token = $state('');

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
		<p class="mx-auto max-w-3xl px-4 py-10 text-slate-700 dark:text-slate-200">{$t('flow.publicShare.missing')}</p>
	{:else}
		{#if cover}
			<div class="h-48 w-full overflow-hidden" data-testid="flow-public-cover">
				{#if gallery}
					<div class="h-[200%] w-full" style:background={gallery} style:transform={`translateY(-${position * 50}%)`}></div>
				{:else if coverUrl}
					<img src={coverUrl} alt="" class="h-[200%] w-full object-cover" style:object-position={`center ${position * 100}%`} />
				{/if}
			</div>
		{/if}
		<article class="flow-canvas prose prose-slate mx-auto max-w-3xl px-16 pt-8 pb-16 dark:prose-invert">
			{#if emoji}
				<p class="text-5xl">{emoji}</p>
			{:else if iconUrl}
				<img src={iconUrl} alt="" class="h-16 w-16 rounded object-cover" />
			{/if}
			<h1 class="mb-6 text-4xl font-semibold text-slate-900 dark:text-slate-100">
				{title || $t('flow.publicShare.untitled')}
			</h1>
			<p class="text-xs text-slate-500">{$t('flow.publicShare.readOnly')}</p>
			{#each blocks as block, index (`${block.kind}-${index}`)}
				{#if block.kind === 'heading'}
					{#if block.level === 1}
						<h1 style={indentStyle(block.indent)}>{block.text}</h1>
					{:else if block.level === 3}
						<h3 style={indentStyle(block.indent)}>{block.text}</h3>
					{:else}
						<h2 style={indentStyle(block.indent)}>{block.text}</h2>
					{/if}
				{:else if block.kind === 'bulletItem'}
					<div class="flow-bullet-item" style={indentStyle(block.indent)}>{block.text}</div>
				{:else if block.kind === 'codeBlock'}
					<pre style={indentStyle(block.indent)}><code>{block.text}</code></pre>
				{:else if block.kind === 'embed'}
					<p class="rounded-md border border-slate-200 px-3 py-2 text-sm text-slate-500 dark:border-slate-700">
						{$t('flow.publicShare.embed')}
					</p>
				{:else}
					<p style={indentStyle(block.indent)}>{block.text}</p>
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
