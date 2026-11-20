<script lang="ts">
	import { t } from 'svelte-i18n';
	import ProgressBar from './ProgressBar.svelte';
</script>

<section data-gallery-section="progress-bar">
	<h2>progress-bar</h2>
	<div class="column">
		<span data-gallery="progress-bar:queued">
			<ProgressBar value={0} label={$t('ui.progress.sample.label')} state="queued" />
		</span>
		<span data-gallery="progress-bar:running">
			<ProgressBar
				value={3}
				max={8}
				label={$t('ui.progress.sample.label')}
				valueText={$t('ui.progress.sample.count')}
				state="running"
			/>
		</span>
		<span data-gallery="progress-bar:done">
			<ProgressBar
				value={100}
				label={$t('ui.progress.sample.label')}
				state="done"
				message={$t('ui.progress.sample.done')}
			/>
		</span>
		<span data-gallery="progress-bar:failed">
			<ProgressBar
				value={40}
				label={$t('ui.progress.sample.label')}
				state="failed"
				message={$t('ui.progress.sample.failed')}
				onretry={() => {}}
			/>
		</span>
	</div>
</section>

<style>
	.column {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		max-width: 360px;
	}
</style>
