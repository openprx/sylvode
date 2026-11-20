<script lang="ts">
	import { t } from 'svelte-i18n';
	import Metric from './Metric.svelte';
</script>

<section data-gallery-section="metric">
	<h2>metric</h2>
	<div class="row">
		<span data-gallery="metric:value">
			<Metric
				label={$t('ui.metric.sample.label')}
				value={1240}
				note={$t('ui.metric.sample.note')}
			/>
		</span>
		<span data-gallery="metric:percent">
			<Metric label={$t('ui.metric.sample.label')} value="98.6%" />
		</span>
		<span data-gallery="metric:empty">
			<Metric label={$t('ui.metric.sample.label')} value={null} />
		</span>
		<span data-gallery="metric:loading">
			<Metric label={$t('ui.metric.sample.label')} value={null} loading />
		</span>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-6);
		align-items: flex-start;
	}
</style>
