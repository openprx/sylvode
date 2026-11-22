<svelte:options runes />

<script lang="ts">
	interface Props {
		/** Width and height in CSS px. The mark is specified down to 16 px; clear space is 4 grid units. */
		size?: number;
		/** Hide from assistive technology when the surrounding link or button already has a name. */
		decorative?: boolean;
	}

	let { size = 28, decorative = false }: Props = $props();
</script>

<svg
	class="brand-mark"
	viewBox="0 0 24 24"
	width={size}
	height={size}
	fill="none"
	stroke="currentColor"
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	role={decorative ? undefined : 'img'}
	aria-label={decorative ? undefined : 'Sylvode'}
	aria-hidden={decorative ? 'true' : undefined}
	focusable="false"
	data-shell="brand-mark"
>
	<path d="M18 5H8a2 2 0 0 0-2 2v3a2 2 0 0 0 2 2h8a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H6" />
</svg>

<style>
	.brand-mark {
		display: block;
		flex: none;
		color: var(--rf-text);
	}
</style>
