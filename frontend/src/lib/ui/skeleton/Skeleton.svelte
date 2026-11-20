<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';

	interface Props extends Omit<HTMLAttributes<HTMLSpanElement>, 'children' | 'class'> {
		/** CSS width, for example "100%" or "180px". */
		width?: string;
		/** CSS height; the bar height follows the text it stands in for. */
		height?: string;
		/** A circle where an avatar will be. */
		circle?: boolean;
		/** Set when the block is the only announcement of a loading area; otherwise it is hidden. */
		label?: string;
	}

	let { width = '100%', height = '12px', circle = false, label, ...rest }: Props = $props();
</script>

<span
	{...rest}
	class="ui-skeleton"
	class:circle
	style:width={circle ? height : width}
	style:height
	role={label ? 'status' : undefined}
	aria-label={label}
	aria-hidden={label ? undefined : 'true'}
	data-ui="skeleton"
></span>

<style>
	/* Blocks in `selected`, radius 4, no animation (specification 3.7 rule 4). */
	.ui-skeleton {
		display: block;
		flex: none;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	.ui-skeleton.circle {
		border-radius: var(--rf-radius-full);
	}
</style>
