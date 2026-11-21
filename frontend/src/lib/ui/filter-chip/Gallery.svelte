<script lang="ts">
	import { t } from 'svelte-i18n';
	import FilterChip from './FilterChip.svelte';
</script>

<section data-gallery-section="filter-chip">
	<h2>filter-chip</h2>
	<div class="row">
		<span data-gallery="filter-chip:off"><FilterChip>{$t('ui.filterChip.sample')}</FilterChip></span
		>
		<span data-gallery="filter-chip:selected"
			><FilterChip selected>{$t('ui.filterChip.sample')}</FilterChip></span
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
