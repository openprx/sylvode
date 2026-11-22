<script lang="ts">
	import { t } from 'svelte-i18n';
	import Pencil from '@lucide/svelte/icons/pencil';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import Button from '../button/Button.svelte';
	import Menu from './Menu.svelte';
	import MenuItem from './MenuItem.svelte';
	import MenuSeparator from './MenuSeparator.svelte';

	let open = $state(false);
	let anchor = $state<HTMLElement | null>(null);
</script>

<section data-gallery-section="menu">
	<h2>menu</h2>
	<span data-gallery="menu:trigger" bind:this={anchor}>
		<Button aria-haspopup="menu" aria-expanded={open} onclick={() => (open = !open)}>
			{$t('ui.gallery.open')}
		</Button>
	</span>
</section>

<Menu {open} {anchor} onclose={() => (open = false)} label={$t('ui.gallery.title')} testId="demo">
	<MenuItem icon={Pencil} shortcut="Ctrl E">{$t('ui.gallery.sample')}</MenuItem>
	<MenuItem icon={Pencil} disabled>{$t('ui.gallery.sample')}</MenuItem>
	<MenuItem compact>{$t('ui.gallery.sample')}</MenuItem>
	<MenuSeparator />
	<MenuItem icon={Trash2} destructive>{$t('ui.gallery.confirm')}</MenuItem>
</Menu>
