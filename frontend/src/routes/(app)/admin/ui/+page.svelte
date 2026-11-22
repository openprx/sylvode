<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { Component } from 'svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';

	// Every component folder in `lib/ui` may hold a `Gallery.svelte`; adding one adds a section
	// here without editing this file.
	const modules = import.meta.glob<{ default: Component }>('/src/lib/ui/**/Gallery.svelte', {
		eager: true
	});
	const sections = Object.entries(modules)
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([path, module]) => ({ path, Section: module.default }));

	let dark = $state(false);
</script>

<!-- Internal test page for the automated checks; not a showcase. -->
<div class="page" class:dark data-theme={dark ? 'dark' : 'light'}>
	<Switch label={$t('ui.gallery.dark')} bind:checked={dark} data-testid="gallery-dark" />
	{#each sections as { path, Section } (path)}
		<Section />
	{/each}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-8);
		min-height: 100vh;
		padding: var(--rf-space-8);
		background: var(--rf-bg);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
</style>
