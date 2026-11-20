<script lang="ts">
	import { BRAND } from './brand';

	interface Props {
		/** Width and height in CSS px. */
		size?: number;
		/** Hide from assistive technology when the surrounding link or button already has a name. */
		decorative?: boolean;
	}

	let { size = 28, decorative = false }: Props = $props();
</script>

<svg
	class="brand-mark"
	viewBox={BRAND.symbol.viewBox}
	width={size}
	height={size}
	fill="currentColor"
	role={decorative ? undefined : 'img'}
	aria-label={decorative ? undefined : BRAND.name}
	aria-hidden={decorative ? 'true' : undefined}
	focusable="false"
	data-shell="brand-mark"
>
	{#each BRAND.symbol.paths as d (d)}
		<path {d} />
	{/each}
</svg>

<style>
	.brand-mark {
		display: block;
		flex: none;
		color: var(--rf-text);
	}
</style>
