<script lang="ts">
	import type { Snippet } from 'svelte';
	import Popover from '../popover/Popover.svelte';
	import { roving } from '../foundations/roving';
	import type { Placement } from '../foundations/position';
	import { provideMenu } from './menu-context';

	interface Props {
		open: boolean;
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Accessible name; or use `labelledby` (usually the trigger's id). */
		label?: string;
		labelledby?: string;
		placement?: Placement;
		width?: number;
		testId?: string;
		children: Snippet;
	}

	let {
		open,
		anchor,
		onclose,
		label,
		labelledby,
		placement = 'bottom-start',
		width = 220,
		testId,
		children
	}: Props = $props();

	const FIRST_ITEM = '[role="menuitem"]:not([aria-disabled="true"])';

	provideMenu({ close: () => onclose() });
</script>

<Popover
	{open}
	{anchor}
	{onclose}
	{label}
	{labelledby}
	{placement}
	{width}
	{testId}
	role="menu"
	padding="menu"
	initialFocus={FIRST_ITEM}
>
	<!-- Arrow keys, Home and End move between items; Tab leaves and closes the menu. -->
	<div class="items" use:roving={{ selector: '[role="menuitem"]', orientation: 'vertical' }}>
		{@render children()}
	</div>
</Popover>

<style>
	.items {
		display: flex;
		flex-direction: column;
	}
</style>
