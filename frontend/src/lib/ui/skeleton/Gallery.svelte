<script lang="ts">
	import { t } from 'svelte-i18n';
	import Skeleton from './Skeleton.svelte';
</script>

<section data-gallery-section="skeleton">
	<h2>skeleton</h2>
	<div class="row">
		<span data-gallery="skeleton:list-row" class="row-shape">
			<Skeleton circle height="40px" />
			<span class="lines">
				<Skeleton width="60%" height="14px" />
				<Skeleton width="85%" height="12px" />
			</span>
		</span>
		<span data-gallery="skeleton:labelled" class="block">
			<Skeleton width="180px" height="16px" label={$t('ui.skeleton.label')} />
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
	.row-shape {
		display: flex;
		align-items: center;
		gap: var(--rf-space-5);
		width: 280px;
	}
	.lines {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		flex: 1;
	}
	.block {
		display: block;
	}
</style>
