<script lang="ts">
	import { t } from 'svelte-i18n';
	import SecurityWarning from './SecurityWarning.svelte';
</script>

<section data-gallery-section="security-warning">
	<h2>security-warning</h2>
	<div class="column">
		<span data-gallery="security-warning:warning">
			<SecurityWarning title={$t('ui.securityWarning.sample.title')}>
				{$t('ui.securityWarning.sample.body')}
			</SecurityWarning>
		</span>
		<span data-gallery="security-warning:danger">
			<SecurityWarning tone="danger" title={$t('ui.securityWarning.sample.dangerTitle')}>
				{$t('ui.securityWarning.sample.dangerBody')}
			</SecurityWarning>
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
