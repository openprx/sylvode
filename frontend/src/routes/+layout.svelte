<script lang="ts">
	import '#lib/i18n/index.js';
	import { onMount } from 'svelte';
	import '../app.css';
	import { faviconDataUrl } from '#lib/shell/brand/brand.js';
	import Toast from '#lib/components/Toast.svelte';
	import { DEFAULT_THEME, applyTheme } from '#lib/theme.js';

	let { children } = $props();

	onMount(() => {
		const saved = localStorage.getItem('theme');
		const nextTheme =
			saved === 'light' || saved === 'dark' || saved === 'system' ? saved : DEFAULT_THEME;
		applyTheme(nextTheme);
	});
</script>

<svelte:head>
	<link rel="icon" href={faviconDataUrl()} />
</svelte:head>

{@render children()}
<Toast />
