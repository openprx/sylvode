<script lang="ts">
	import { t } from 'svelte-i18n';
	import Checkbox from './Checkbox.svelte';
</script>

<section data-gallery-section="checkbox">
	<h2>checkbox</h2>
	<div class="row">
		<span data-gallery="checkbox:off"><Checkbox label={$t('ui.gallery.sample')} /></span>
		<span data-gallery="checkbox:on"><Checkbox label={$t('ui.gallery.sample')} checked /></span>
		<span data-gallery="checkbox:indeterminate">
			<Checkbox label={$t('ui.gallery.sample')} indeterminate />
		</span>
		<span data-gallery="checkbox:disabled">
			<Checkbox label={$t('ui.gallery.sample')} disabled />
		</span>
		<span data-gallery="checkbox:disabled-checked">
			<Checkbox label={$t('ui.gallery.sample')} checked disabled />
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
