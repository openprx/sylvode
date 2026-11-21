<script lang="ts">
	import { t } from 'svelte-i18n';
	import Switch from './Switch.svelte';
</script>

<section data-gallery-section="switch">
	<h2>switch</h2>
	<div class="row">
		<span data-gallery="switch:off"><Switch label={$t('ui.gallery.sample')} /></span>
		<span data-gallery="switch:on"><Switch label={$t('ui.gallery.sample')} checked /></span>
		<span data-gallery="switch:disabled"><Switch label={$t('ui.gallery.sample')} disabled /></span>
		<span data-gallery="switch:disabled-on">
			<Switch label={$t('ui.gallery.sample')} checked disabled />
		</span>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-6);
	}
</style>
