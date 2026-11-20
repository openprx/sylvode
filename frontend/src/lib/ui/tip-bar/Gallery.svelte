<script lang="ts">
	import { t } from 'svelte-i18n';
	import TipBar from './TipBar.svelte';
</script>

<section data-gallery-section="tip-bar">
	<h2>tip-bar</h2>
	<div class="column">
		<span data-gallery="tip-bar:neutral"><TipBar>{$t('ui.tipBar.sample.neutral')}</TipBar></span>
		<span data-gallery="tip-bar:success"
			><TipBar tone="success">{$t('ui.tipBar.sample.success')}</TipBar></span
		>
		<span data-gallery="tip-bar:warning">
			<TipBar tone="warning" action={{ label: $t('ui.tipBar.sample.action'), run: () => {} }}>
				{$t('ui.tipBar.sample.warning')}
			</TipBar>
		</span>
		<span data-gallery="tip-bar:danger">
			<TipBar tone="danger" action={{ label: $t('ui.tipBar.sample.action'), run: () => {} }}>
				{$t('ui.tipBar.sample.danger')}
			</TipBar>
		</span>
	</div>
</section>

<style>
	.column {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		max-width: 480px;
	}
</style>
