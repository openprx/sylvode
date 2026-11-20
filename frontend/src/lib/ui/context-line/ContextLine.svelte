<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import { plural } from '../foundations/plural';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Lock from '@lucide/svelte/icons/lock';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Menu from '../menu/Menu.svelte';
	import MenuItem from '../menu/MenuItem.svelte';
	import { tooltip } from '../tooltip/tooltip';
	import { uid } from '../foundations/ids';
	import { collapseSegments, isTruncated, type ContextSegment } from './context-line';

	interface Props {
		/** Ancestors, in order. */
		segments: ContextSegment[];
		/** Label of the current item. */
		current: string;
		loading?: boolean;
		/** Accessible name; default is the localised "Location". */
		label?: string;
		/** Id for the element, so a title can reference it with aria-describedby. */
		descriptionId?: string;
		/** Width the line may use; measured from the element when absent. */
		width?: number;
	}

	let { segments, current, loading = false, label, descriptionId, width }: Props = $props();

	const ellipsisWidthFallback = 28;
	let nav = $state<HTMLElement | null>(null);
	let measure = $state<HTMLElement | null>(null);
	let measuredWidth = $state(0);
	let natural = $state<{ segments: number[]; current: number; ellipsis: number } | null>(null);
	let menuOpen = $state(false);
	let menuAnchor = $state<HTMLElement | null>(null);
	let truncated = $state<Record<string, boolean>>({});
	const menuId = uid('context-menu');

	const available = $derived(width ?? measuredWidth);
	const hidden = $derived(
		natural && available > 0
			? collapseSegments({
					segments: natural.segments,
					current: natural.current,
					ellipsis: natural.ellipsis || ellipsisWidthFallback,
					available
				})
			: []
	);
	const hiddenSegments = $derived(hidden.map((index) => segments[index]));
	const stateOf = (s: ContextSegment) => s.state ?? 'normal';
	const labelOf = (s: ContextSegment) =>
		stateOf(s) === 'restricted' ? $t('ui.contextLine.restricted') : s.label;

	$effect(() => {
		if (!nav || width !== undefined) return;
		const observer = new ResizeObserver(() => (measuredWidth = nav!.clientWidth));
		observer.observe(nav);
		measuredWidth = nav.clientWidth;
		return () => observer.disconnect();
	});

	// The hidden twin list holds every segment at its natural width; it is measured again when
	// the content changes.
	$effect(() => {
		void segments;
		void current;
		if (!measure) return;
		const items = Array.from(measure.querySelectorAll<HTMLElement>(':scope > li'));
		const widths = items.map((item) => item.getBoundingClientRect().width);
		natural = {
			segments: widths.slice(0, segments.length),
			current: widths[segments.length] ?? 0,
			ellipsis: widths[segments.length + 1] ?? 0
		};
	});

	function noteTruncation(id: string, el: HTMLElement) {
		truncated[id] = isTruncated(el);
	}
	function onEllipsisKey(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			menuOpen = true;
		}
	}
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- segment hrefs are full paths built by the caller -->
{#snippet item(segment: ContextSegment)}
	{@const state = stateOf(segment)}
	{#if state === 'restricted'}
		<span
			class="plain"
			aria-label={$t('ui.contextLine.noAccess', { values: { label: labelOf(segment) } })}
		>
			<Lock size={14} aria-hidden="true" />
			<span class="text">{labelOf(segment)}</span>
		</span>
	{:else if state === 'unavailable' || !segment.href}
		<span class="plain">
			{#if state === 'unavailable'}<CircleAlert size={14} aria-hidden="true" />{/if}
			<span class="text"
				>{state === 'unavailable' ? $t('ui.contextLine.unavailable') : segment.label}</span
			>
		</span>
	{:else}
		<a
			class="rf-link link"
			href={segment.href}
			data-segment={segment.id}
			use:tooltip={{ text: truncated[segment.id] ? segment.label : '' }}
			onpointerenter={(e) => noteTruncation(segment.id, e.currentTarget)}
			onfocus={(e) => noteTruncation(segment.id, e.currentTarget)}>{segment.label}</a
		>
	{/if}
{/snippet}

<div class="ui-context-line" data-ui="context-line">
	<nav
		bind:this={nav}
		id={descriptionId}
		class="nav"
		aria-label={label ?? $t('ui.contextLine.label')}
	>
		{#if loading}
			<span class="skeleton" role="status" aria-label={$t('ui.contextLine.loading')}></span>
		{:else}
			<ol class="list">
				{#each segments as segment, index (segment.id)}
					{#if index === 1 && hidden.length > 0}
						<li class="entry more-entry">
							<button
								bind:this={menuAnchor}
								type="button"
								class="more"
								aria-haspopup="menu"
								aria-expanded={menuOpen}
								aria-controls={menuOpen ? menuId : undefined}
								aria-label={plural($t, 'ui.contextLine.showHidden', hidden.length, $locale)}
								onclick={() => (menuOpen = !menuOpen)}
								onkeydown={onEllipsisKey}
							>
								<Ellipsis size={14} aria-hidden="true" />
							</button>
						</li>
					{/if}
					{#if !hidden.includes(index)}
						<li class="entry">{@render item(segment)}</li>
					{/if}
				{/each}
				<li class="entry current" aria-current="page">
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span
						class="text"
						use:tooltip={{ text: truncated.__current ? current : '' }}
						onpointerenter={(e) => noteTruncation('__current', e.currentTarget)}>{current}</span
					>
				</li>
			</ol>
		{/if}
	</nav>

	<!-- Natural-width twin used only to measure; never exposed. -->
	<ol class="list measure" bind:this={measure} aria-hidden="true" inert>
		{#each segments as segment (segment.id)}
			<li class="entry">{@render item(segment)}</li>
		{/each}
		<li class="entry current"><span class="text">{current}</span></li>
		<li class="entry more-entry">
			<span class="more"><Ellipsis size={14} aria-hidden="true" /></span>
		</li>
	</ol>
</div>

<Menu
	open={menuOpen}
	anchor={menuAnchor}
	onclose={() => (menuOpen = false)}
	label={$t('ui.contextLine.label')}
	width={220}
	testId="context-line-menu"
>
	{#each hiddenSegments as segment (segment.id)}
		<MenuItem compact disabled={!segment.href || stateOf(segment) !== 'normal'} href={segment.href}>
			{labelOf(segment)}
		</MenuItem>
	{/each}
</Menu>

<style>
	.ui-context-line {
		position: relative;
		min-width: 0;
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: 18px;
		color: var(--rf-secondary);
	}
	.nav {
		min-width: 0;
	}
	.list {
		display: flex;
		align-items: center;
		min-width: 0;
		margin: 0;
		padding: 0;
		list-style: none;
		white-space: nowrap;
	}
	.measure {
		position: absolute;
		inset-inline-start: 0;
		top: 0;
		visibility: hidden;
		pointer-events: none;
		width: max-content;
	}
	.entry {
		display: flex;
		align-items: center;
		min-width: 0;
		flex: 0 1 auto;
	}
	.entry + .entry::before {
		content: '/';
		flex: none;
		padding: 0 var(--rf-space-2);
		color: var(--rf-secondary);
	}
	.entry:not(.current):not(.more-entry) {
		min-width: 48px;
	}
	.link,
	.plain {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		min-width: 0;
		max-width: 240px;
		color: var(--rf-secondary);
	}
	.link {
		overflow: hidden;
		text-overflow: ellipsis;
		text-decoration: none;
	}
	.link:hover {
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.link:active {
		color: var(--rf-text);
		text-decoration: underline;
	}
	.text {
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.current {
		flex-shrink: 1;
		min-width: 80px;
		font-weight: var(--rf-weight-medium);
		color: var(--rf-text);
	}
	/* Proposed (batch A): an inline text link keeps its ring outside the glyphs. */
	.rf-link:focus-visible,
	.link:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
		border-radius: 2px;
	}
	.more {
		display: inline-grid;
		place-items: center;
		min-width: 24px;
		height: 20px;
		padding: 0 var(--rf-space-2);
		border: 0;
		border-radius: var(--rf-radius-xs);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.more:hover {
		background: var(--rf-hover);
	}
	.more:active,
	.more[aria-expanded='true'] {
		background: var(--rf-pressed);
	}
	.more:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.skeleton {
		display: block;
		width: 160px;
		height: 12px;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	@media (pointer: coarse) {
		.link,
		.more {
			padding-block: 13px;
			margin-block: -13px;
		}
	}
</style>
