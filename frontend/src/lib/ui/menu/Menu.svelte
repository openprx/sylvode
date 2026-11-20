<script lang="ts">
	import type { Snippet } from 'svelte';
	import Popover from '../popover/Popover.svelte';
	import { roving } from '../foundations/roving';
	import { TypeaheadBuffer, isTypeaheadKey, typeaheadMatch } from '../foundations/typeahead';
	import type { Placement } from '../foundations/position';
	import { provideMenu } from './menu-context';

	/** Data attributes (`data-testid`, ...) are passed to the menu surface. */
	type DataAttributes = { [K in `data-${string}`]?: string | undefined };

	interface Props extends DataAttributes {
		open: boolean;
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Accessible name; or use `labelledby` (usually the trigger's id). */
		label?: string;
		labelledby?: string;
		describedby?: string;
		placement?: Placement;
		width?: number;
		testId?: string;
		/** Non-interactive heading content above the items (a caption); inside the menu surface. */
		header?: Snippet;
		children: Snippet;
	}

	let {
		open,
		anchor,
		onclose,
		label,
		labelledby,
		describedby,
		placement = 'bottom-start',
		width = 220,
		testId,
		header,
		children,
		...rest
	}: Props = $props();

	const ITEMS = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]';
	const FIRST_ITEM = ITEMS.split(', ')
		.map((part) => `${part}:not([aria-disabled="true"])`)
		.join(', ');

	const buffer = new TypeaheadBuffer();

	/** First-letter navigation: a printable key focuses the next item that starts with it. */
	function onKeydown(event: KeyboardEvent) {
		if (!isTypeaheadKey(event)) return;
		const typed = buffer.push(event.key);
		// Space activates the focused item unless a search is already under way.
		if (event.key === ' ' && typed === ' ') {
			buffer.reset();
			return;
		}
		const container = event.currentTarget as HTMLElement;
		const items = [...container.querySelectorAll<HTMLElement>(ITEMS)];
		const labels = items.map((el) => (el.querySelector('.label') ?? el).textContent ?? '');
		const current = items.indexOf(document.activeElement as HTMLElement);
		const index = typeaheadMatch(labels, current, typed);
		if (index === null) return;
		event.preventDefault();
		items[index].focus();
	}

	provideMenu({ close: () => onclose() });
</script>

<Popover
	{open}
	{anchor}
	{onclose}
	{label}
	{labelledby}
	{describedby}
	{placement}
	{width}
	{testId}
	{...rest}
	role="menu"
	padding="menu"
	initialFocus={FIRST_ITEM}
>
	<!-- Arrow keys, Home and End move between items; Tab leaves and closes the menu. -->
	{#if header}{@render header()}{/if}
	<div
		class="items"
		role="none"
		use:roving={{ selector: ITEMS, orientation: 'vertical' }}
		onkeydown={onKeydown}
	>
		{@render children()}
	</div>
</Popover>

<style>
	.items {
		display: flex;
		flex-direction: column;
	}
</style>
