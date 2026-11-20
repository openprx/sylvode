<script lang="ts">
	import { t } from 'svelte-i18n';
	import TextField from './TextField.svelte';

	let value = $state('');
</script>

<section data-gallery-section="text-field">
	<h2>text-field</h2>
	<div class="col">
		<div data-gallery="text-field:default">
			<TextField label={$t('ui.gallery.field')} bind:value placeholder={$t('ui.gallery.sample')} />
		</div>
		<div data-gallery="text-field:helper">
			<TextField label={$t('ui.gallery.field')} helper={$t('ui.gallery.sample')} />
		</div>
		<div data-gallery="text-field:invalid">
			<TextField label={$t('ui.gallery.field')} value="x" error={$t('ui.gallery.reason')} />
		</div>
		<div data-gallery="text-field:disabled">
			<TextField label={$t('ui.gallery.field')} value="x" disabled />
		</div>
		<div data-gallery="text-field:readonly">
			<TextField label={$t('ui.gallery.field')} value="x" readonly />
		</div>
	</div>
</section>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 320px;
	}
</style>
