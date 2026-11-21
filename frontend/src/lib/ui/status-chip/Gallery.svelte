<script lang="ts">
	import { t } from 'svelte-i18n';
	import StatusChip from './StatusChip.svelte';
</script>

<section data-gallery-section="status-chip">
	<h2>status-chip</h2>
	<div class="row">
		<span data-gallery="status-chip:neutral"
			><StatusChip>{$t('ui.statusChip.neutral')}</StatusChip></span
		>
		<span data-gallery="status-chip:success"
			><StatusChip tone="success">{$t('ui.statusChip.success')}</StatusChip></span
		>
		<span data-gallery="status-chip:warning"
			><StatusChip tone="warning">{$t('ui.statusChip.warning')}</StatusChip></span
		>
		<span data-gallery="status-chip:danger"
			><StatusChip tone="danger">{$t('ui.statusChip.danger')}</StatusChip></span
		>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
</style>
