<script lang="ts">
	import type { Snippet } from 'svelte';
	import Popover from '../popover/Popover.svelte';

	interface Props {
		open: boolean;
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Names the popover for assistive technology; the info button's label. */
		label: string;
		children: Snippet;
	}

	let { open, anchor, onclose, label, children }: Props = $props();
</script>

<Popover {open} {anchor} {onclose} {label} role="dialog" width={300} padding="default">
	<div class="info-body" data-ui="info-popover">
		{@render children()}
	</div>
</Popover>

<style>
	.info-body {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
	}
</style>
