<script lang="ts">
	import Skeleton from '#lib/ui/skeleton/Skeleton.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';

	interface Props {
		/** Translated plural name of what loads ("projects"). */
		thing: string;
		count?: number;
	}
	let { thing, count = 3 }: Props = $props();
</script>

<div class="skeleton" data-column-skeleton>
	<StateBlock kind="loading" {thing} />
	{#each Array.from({ length: count }, (_, i) => i) as index (index)}
		<div class="row"><Skeleton height="16px" /></div>
	{/each}
</div>

<style>
	.skeleton {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
	}
	.row {
		display: flex;
		align-items: center;
		min-height: var(--rf-space-9);
		padding-inline: var(--rf-space-4);
	}
</style>
