<script lang="ts">
	import { randomUuid } from '#lib/utils/uuid.js';
	/**
	 * Frame of an embedded surface: no rail, column, tab strip or header. It is the hosted side of
	 * the surface contract: it receives the host's messages, applies its theme and density to the
	 * document, measures the document width, and gives surfaces an `EmbedSurfaceHost`.
	 */
	import '#lib/i18n/index.js';
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { locale } from 'svelte-i18n';
	import { provideSurfaceHostProvider } from '#lib/surface/context.js';
	import {
		EMBED_INIT_TIMEOUT_MS,
		FRAME_ID_PATTERN,
		embedOrigins
	} from '#lib/surface/embed-config.js';
	import { EmbedSurfaceHost } from '#lib/surface/embed-host.svelte.js';
	import { surfaceKindOf } from '#lib/surface/registry.js';

	let { children }: { children: Snippet } = $props();

	const origins = embedOrigins();
	const framed = window.parent !== window;
	const requested = page.url.searchParams.get('frame') ?? '';
	const frameId = FRAME_ID_PATTERN.test(requested) ? requested : randomUuid();

	const host = new EmbedSurfaceHost({
		parent: window.parent,
		origins,
		frameId,
		systemTheme: window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light',
		// Only a frame inside another page has anyone to tell; the target origin is always its own.
		post: (message) => {
			if (framed && origins[0]) window.parent.postMessage(message, origins[0]);
		},
		onlocale: (next) => {
			locale.set(next);
			document.documentElement.lang = next;
		},
		writeUrl: (query, mode) => {
			const url = new URL(page.url.href);
			for (const [key, value] of Object.entries(query)) {
				if (value === null) url.searchParams.delete(key);
				else url.searchParams.set(key, value);
			}
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- same route, query only
			void goto(url.pathname + url.search, {
				replace: mode !== 'push',
				reset: false
			});
		}
	});
	provideSurfaceHostProvider(host);

	let frameElement = $state<HTMLElement | null>(null);

	// Theme and density belong to the host (specification 6, rule 3). The app's root layout applies
	// the stored theme after this runs once, so the host's value is applied again after it.
	$effect(() => {
		const { theme, density } = host;
		const apply = () => {
			const root = document.documentElement;
			root.classList.toggle('dark', theme === 'dark');
			root.dataset.theme = theme;
			root.dataset.density = density;
		};
		apply();
		queueMicrotask(apply);
	});

	$effect(() => {
		if (host.slot !== 'inline' || !frameElement) return;
		const element = frameElement;
		const observer = new ResizeObserver(() => host.reportHeight(element.offsetHeight));
		observer.observe(element);
		return () => observer.disconnect();
	});

	onMount(() => {
		const root = document.documentElement;
		host.setMeasuredWidth(root.clientWidth);
		const observer = new ResizeObserver(() => host.setMeasuredWidth(root.clientWidth));
		observer.observe(root);
		const onMessage = (event: MessageEvent) => host.receive(event);
		window.addEventListener('message', onMessage);
		host.announce(surfaceKindOf(new URL(page.url.href)) ?? 'unknown');
		// A page opened directly has no host to wait for; a frame waits a moment for `sv:init`.
		const timer = framed ? setTimeout(() => host.settle(), EMBED_INIT_TIMEOUT_MS) : null;
		if (!framed) host.settle();
		return () => {
			observer.disconnect();
			window.removeEventListener('message', onMessage);
			if (timer) clearTimeout(timer);
		};
	});

	// The host owns Escape: an Escape nothing inside the frame handled becomes a close request.
	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape' || event.defaultPrevented || !framed) return;
		host.requestClose();
	}

	// A hosted surface never navigates the window: links a surface did not handle open elsewhere
	// (web addresses in a new tab) or do nothing (addresses of this app).
	function onClick(event: MouseEvent) {
		if (event.defaultPrevented || !(event.target instanceof Element)) return;
		const anchor = event.target.closest('a[href]');
		if (!(anchor instanceof HTMLAnchorElement) || anchor.target === '_blank') return;
		let url: URL;
		try {
			url = new URL(anchor.href);
		} catch {
			return;
		}
		if (url.protocol !== 'http:' && url.protocol !== 'https:') return;
		event.preventDefault();
		if (url.origin !== location.origin) window.open(url.href, '_blank', 'noopener,noreferrer');
	}
</script>

<svelte:window onkeydown={onKeydown} onclick={onClick} />

<div
	class="embed-frame"
	bind:this={frameElement}
	data-embed-frame
	data-surface-width={host.widthClass}
	data-surface-slot={host.slot}
>
	{#if host.settled}{@render children()}{/if}
</div>

<style>
	.embed-frame {
		container-type: inline-size;
		container-name: surface;
		min-width: 0;
		background: var(--rf-bg);
		color: var(--rf-text);
	}
</style>
