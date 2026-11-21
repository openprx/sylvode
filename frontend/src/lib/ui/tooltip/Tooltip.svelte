<script lang="ts">
	import { autoPosition } from '../foundations/position';
	import { portal } from '../foundations/portal';

	interface Props {
		id: string;
		anchor: HTMLElement;
		text: string;
		shortcut?: string;
		/** Hidden from assistive technology when it only repeats the anchor's name. */
		describes?: boolean;
		onpointerenter?: () => void;
		onpointerleave?: () => void;
	}

	let {
		id,
		anchor,
		text,
		shortcut,
		describes = false,
		onpointerenter,
		onpointerleave
	}: Props = $props();

	let node = $state<HTMLElement | null>(null);

	$effect(() => {
		if (!node) return;
		return autoPosition(anchor, node, { placement: 'bottom', offset: 6, margin: 8 });
	});
</script>

<div
	bind:this={node}
	{id}
	role="tooltip"
	aria-hidden={describes ? undefined : 'true'}
	class="ui-tooltip"
	use:portal={anchor}
	{onpointerenter}
	{onpointerleave}
>
	<span>{text}</span>
	{#if shortcut}<span class="shortcut">{shortcut}</span>{/if}
</div>

<style>
	.ui-tooltip {
		position: fixed;
		z-index: var(--rf-z-tooltip);
		left: 0;
		top: 0;
		display: flex;
		gap: var(--rf-space-4);
		max-width: 240px;
		padding: var(--rf-space-2) var(--rf-space-4);
		border-radius: var(--rf-radius-chip);
		background: var(--rf-tooltip);
		color: var(--rf-on-tooltip);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		animation: ui-tooltip-in var(--rf-duration-base) var(--rf-ease-standard);
	}
	@keyframes ui-tooltip-in {
		from {
			opacity: 0;
		}
	}
</style>
