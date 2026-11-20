<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import Popover from './Popover.svelte';

	let open = $state(false);
	let anchor = $state<HTMLElement | null>(null);
</script>

<section data-gallery-section="popover">
	<h2>popover</h2>
	<span data-gallery="popover:trigger" bind:this={anchor}>
		<Button aria-expanded={open} onclick={() => (open = !open)}>{$t('ui.gallery.open')}</Button>
	</span>
</section>

<Popover
	{open}
	{anchor}
	onclose={() => (open = false)}
	label={$t('ui.gallery.title')}
	testId="demo"
>
	<p>{$t('ui.gallery.body')}</p>
	<Button onclick={() => (open = false)}>{$t('ui.dialog.close')}</Button>
</Popover>
