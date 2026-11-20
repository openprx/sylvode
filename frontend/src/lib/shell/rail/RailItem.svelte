<script lang="ts">
	import { tooltip } from '../../ui/tooltip/tooltip';
	import { uid } from '../../ui/foundations/ids';
	import { glyphFor } from './glyphs';
	import type { RailItemModel } from './rail-model';

	interface Props {
		item: RailItemModel;
		/** Layout of the bar the item sits in: a vertical rail tile or a top-bar text link. */
		layout?: 'rail' | 'topbar';
		/** Set only on the selected destination: whether its column is open. */
		columnExpanded?: boolean;
		/** Roving tab stop. */
		tabindex?: 0 | -1;
		/** Called for a plain click on a link; the handler may call `preventDefault()`. */
		onactivate?: (item: RailItemModel, event: MouseEvent) => void;
	}

	let { item, layout = 'rail', columnExpanded, tabindex = 0, onactivate }: Props = $props();

	const reasonId = uid('rail-reason');
	const Glyph = $derived(glyphFor(item.glyph));
	const isDestination = $derived(item.kind === 'destination');
	const dataAttrs = $derived(
		isDestination
			? { 'data-rail-destination': item.id }
			: { 'data-rail-shortcut': item.id, 'data-shortcut-kind': item.shortcutKind }
	);
	const named = $derived(item.accessibleName !== item.label);

	function click(event: MouseEvent) {
		if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0)
			return;
		onactivate?.(item, event);
	}
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- hrefs are full in-app paths built by the shell model -->

{#snippet face()}
	{#if layout === 'rail'}
		<span class="glyph">
			<Glyph size={20} aria-hidden="true" />
			{#if item.badge}<span class="badge" aria-hidden="true" data-shell="rail-badge"
					>{item.badge}</span
				>{/if}
		</span>
		<span class="label">{item.label}</span>
	{:else}
		<span class="label">{item.label}</span>
		{#if item.badge}<span class="badge in-line" aria-hidden="true" data-shell="rail-badge"
				>{item.badge}</span
			>{/if}
	{/if}
{/snippet}

{#if item.href}
	<a
		class="rail-item layout-{layout}"
		href={item.href}
		{...dataAttrs}
		{tabindex}
		aria-current={item.selected ? 'page' : undefined}
		aria-expanded={item.selected && columnExpanded !== undefined ? columnExpanded : undefined}
		aria-label={named ? item.accessibleName : undefined}
		onclick={click}
		use:tooltip={{ text: item.label }}
	>
		{@render face()}
	</a>
{:else}
	<!-- Unavailable: visible at reduced strength, not a link; the reason is its description. -->
	<button
		type="button"
		class="rail-item layout-{layout} unavailable"
		aria-disabled="true"
		{...dataAttrs}
		{tabindex}
		aria-describedby={reasonId}
		use:tooltip={{ text: item.unavailableReason ?? '' }}
	>
		{@render face()}
		<span id={reasonId} class="reason">{item.unavailableReason}</span>
	</button>
{/if}

<style>
	.rail-item {
		position: relative;
		padding: 0;
		border: 0;
		background: transparent;
		display: flex;
		align-items: center;
		justify-content: center;
		box-sizing: border-box;
		color: var(--rf-text);
		text-decoration: none;
		border-radius: var(--rf-radius-sm);
		font-family: var(--rf-font-sans-stack);
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.rail-item:hover {
		background: var(--rf-hover-rail);
	}
	.rail-item:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.rail-item[aria-current='page'] {
		background: var(--rf-selected);
	}
	.unavailable {
		opacity: var(--rf-disabled-opacity);
		cursor: not-allowed;
	}
	.unavailable:hover {
		background: transparent;
	}
	.reason {
		position: absolute;
		width: 1px;
		height: 1px;
		margin: -1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}

	/* Rail tile: 52 square, glyph over label. */
	.layout-rail {
		flex-direction: column;
		gap: var(--rf-space-2);
		width: 52px;
		height: 52px;
		flex: none;
	}
	.layout-rail[aria-current='page']::before {
		content: '';
		position: absolute;
		inset-inline-start: calc(-1 * var(--rf-space-4));
		top: 50%;
		width: var(--rf-border-width-focus);
		height: 26px;
		transform: translateY(-50%);
		background: var(--rf-primary);
	}
	.glyph {
		position: relative;
		display: grid;
		place-items: center;
		width: 20px;
		height: 20px;
	}
	.layout-rail .label {
		max-width: 100%;
		padding-inline: var(--rf-space-1);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--rf-size-caption);
		line-height: 1.2;
		font-weight: var(--rf-weight-medium);
	}
	.badge {
		position: absolute;
		inset-block-start: -8px;
		inset-inline-start: 10px;
		display: inline-grid;
		place-items: center;
		box-sizing: border-box;
		min-width: 16px;
		height: 16px;
		padding-inline: var(--rf-space-2);
		border-radius: var(--rf-radius-full);
		background: var(--rf-primary);
		color: var(--rf-on-primary);
		font-size: var(--rf-size-caption);
		font-weight: var(--rf-weight-semibold);
		line-height: 1;
		font-variant-numeric: tabular-nums;
	}
	.badge.in-line {
		position: static;
		margin-inline-start: var(--rf-space-2);
	}

	/* Top bar: a text link 44 high. */
	.layout-topbar {
		flex: none;
		height: 44px;
		padding: 0 var(--rf-space-5);
	}
	.layout-topbar .label {
		font-size: var(--rf-size-button);
		font-weight: var(--rf-weight-medium);
		white-space: nowrap;
	}
	.layout-topbar[aria-current='page']::after {
		content: '';
		position: absolute;
		inset-inline: var(--rf-space-5);
		inset-block-end: 0;
		height: var(--rf-border-width-focus);
		background: var(--rf-primary);
	}
</style>
