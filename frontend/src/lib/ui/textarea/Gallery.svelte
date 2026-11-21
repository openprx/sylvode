<script lang="ts">
	import { t } from 'svelte-i18n';
	import Textarea from './Textarea.svelte';
</script>

<section data-gallery-section="textarea">
	<h2>textarea</h2>
	<div class="col">
		<div data-gallery="textarea:default">
			<Textarea label={$t('ui.gallery.field')} placeholder={$t('ui.gallery.sample')} />
		</div>
		<div data-gallery="textarea:invalid">
			<Textarea label={$t('ui.gallery.field')} value="x" error={$t('ui.gallery.reason')} />
		</div>
		<div data-gallery="textarea:disabled">
			<Textarea label={$t('ui.gallery.field')} value="x" disabled />
		</div>
		<div data-gallery="textarea:readonly">
			<Textarea label={$t('ui.gallery.field')} value="x" readonly />
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
