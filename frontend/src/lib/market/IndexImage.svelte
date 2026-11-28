<svelte:options runes />

<script lang="ts">
	import { onDestroy } from 'svelte';
	import { t } from 'svelte-i18n';
	import ImageOff from '@lucide/svelte/icons/image-off';
	import Puzzle from '@lucide/svelte/icons/puzzle';
	import { indexApi } from './index-api';

	/**
	 * One image of a plugin (icon or screenshot). The asset route needs the console's login, so the
	 * bytes are fetched with it and shown from an object URL; the image is requested when it comes
	 * near the viewport. While it loads a quiet block holds the space; if it cannot be fetched the
	 * block says so (screenshot) or shows the plugin glyph (icon), and the layout does not move.
	 */
	let {
		workspaceId,
		source,
		pluginId,
		path,
		alt,
		variant,
		fit = 'cover'
	}: {
		workspaceId: string;
		source: string;
		pluginId: string;
		path: string;
		alt: string;
		variant: 'icon' | 'screenshot';
		fit?: 'cover' | 'contain';
	} = $props();

	type State = 'idle' | 'loading' | 'ready' | 'failed';
	let phase = $state<State>('idle');
	let url = $state<string | null>(null);
	let host = $state<HTMLElement | null>(null);
	let visible = $state(false);

	$effect(() => {
		if (!host) return;
		if (typeof IntersectionObserver === 'undefined') {
			visible = true;
			return;
		}
		const observer = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) {
					visible = true;
					observer.disconnect();
				}
			},
			{ rootMargin: '200px' }
		);
		observer.observe(host);
		return () => observer.disconnect();
	});

	$effect(() => {
		// The identity of the image; a new one starts over.
		void [workspaceId, source, pluginId, path];
		if (!visible) return;
		const controller = new AbortController();
		phase = 'loading';
		let made: string | null = null;
		void indexApi.asset(workspaceId, source, pluginId, path, controller.signal).then((result) => {
			if (controller.signal.aborted) return;
			if (result.code !== 0 || !result.data) {
				phase = 'failed';
				return;
			}
			made = URL.createObjectURL(result.data.blob);
			url = made;
			phase = 'ready';
		});
		return () => {
			controller.abort();
			if (made) URL.revokeObjectURL(made);
			url = null;
		};
	});

	onDestroy(() => {
		url = null;
	});
</script>

<span class="index-image v-{variant}" bind:this={host} data-state={phase}>
	{#if phase === 'ready' && url}
		<img src={url} {alt} class="fit-{fit}" decoding="async" onerror={() => (phase = 'failed')} />
	{:else if phase === 'failed'}
		{#if variant === 'icon'}
			<Puzzle size={22} aria-hidden="true" />
		{:else}
			<span class="failed"
				><ImageOff size={20} aria-hidden="true" />{$t('market.index.imageFailed')}</span
			>
		{/if}
	{:else if variant === 'icon'}
		<Puzzle size={22} aria-hidden="true" class="ghost" />
	{/if}
</span>

<style>
	.index-image {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		overflow: hidden;
		background: var(--rf-surface);
		color: var(--rf-secondary);
		border: var(--rf-border-width) solid var(--rf-line);
		flex: none;
	}
	.v-icon {
		width: 48px;
		height: 48px;
		border-radius: var(--rf-radius-card);
	}
	.v-screenshot {
		width: 100%;
		aspect-ratio: 16 / 10;
		border-radius: var(--rf-radius-card);
	}
	.index-image[data-state='loading'] {
		animation: pulse 1.4s ease-in-out infinite;
	}
	.index-image :global(.ghost) {
		opacity: 0.4;
	}
	img {
		width: 100%;
		height: 100%;
	}
	.fit-cover {
		object-fit: cover;
	}
	.fit-contain {
		object-fit: contain;
	}
	.failed {
		display: inline-flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-3);
		font-size: var(--rf-size-meta);
		padding: var(--rf-space-5);
		text-align: center;
	}
	@keyframes pulse {
		50% {
			opacity: 0.6;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.index-image[data-state='loading'] {
			animation: none;
		}
	}
</style>
