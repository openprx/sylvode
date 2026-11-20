<script lang="ts">
	import { t } from 'svelte-i18n';
	import InfoButton from './InfoButton.svelte';
</script>

<section data-gallery-section="info-button">
	<h2>info-button</h2>
	<div class="row">
		<span data-gallery="info-button:default">
			<InfoButton label={$t('ui.infoButton.sample.label')}>
				{$t('ui.infoButton.sample.body')}
			</InfoButton>
		</span>
	</div>
</section>

<style>
	.row {
		display: flex;
		align-items: center;
	}
</style>
