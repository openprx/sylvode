<script lang="ts">
	import { t, locale } from 'svelte-i18n';
	import type { HTMLAttributes } from 'svelte/elements';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import { uid } from '../foundations/ids';
	import Select from '../select/Select.svelte';
	import {
		focusAfterPage,
		layoutFor,
		pageModel,
		positionText,
		rangeText,
		type Translate
	} from './pagination-state';

	interface Props extends Omit<HTMLAttributes<HTMLElement>, 'class' | 'children'> {
		page: number;
		pageSize: number;
		total: number | null;
		hasMore?: boolean;
		/** With an unknown total: rows on the current page, so the range ends where the data ends. */
		shown?: number;
		pageSizes?: number[];
		loading?: boolean;
		onpage: (page: number) => void;
		onpagesize?: (size: number) => void;
		label?: string;
		density?: 'comfortable' | 'compact';
	}

	let {
		page,
		pageSize,
		total,
		hasMore = false,
		shown,
		pageSizes = [20, 50, 100],
		loading = false,
		onpage,
		onpagesize,
		label,
		density = 'comfortable',
		...rest
	}: Props = $props();

	const sizeId = uid('page-size');
	let width = $state(0);
	let prevButton = $state<HTMLButtonElement | null>(null);
	let nextButton = $state<HTMLButtonElement | null>(null);
	let pending: 'prev' | 'next' | null = null;

	const model = $derived(pageModel({ page, pageSize, total, hasMore, shown }));
	// Before the first measurement the full form is shown (also what the server renders).
	const layout = $derived(layoutFor(width || Number.POSITIVE_INFINITY));
	const translate = $derived<Translate>((key, values) => $t(key, { values }));
	const range = $derived(
		rangeText(model, { loading, short: layout.short, locale: $locale }, translate)
	);
	const position = $derived(
		positionText(model, { short: layout.short, locale: $locale }, translate)
	);
	const prevName = $derived(
		$t(model.isFirst ? 'ui.pagination.previousFirst' : 'ui.pagination.previous')
	);
	const nextName = $derived($t(model.isLast ? 'ui.pagination.nextLast' : 'ui.pagination.next'));
	const sizeOptions = $derived(pageSizes.map((n) => ({ value: String(n), label: String(n) })));

	function go(direction: 'prev' | 'next') {
		if (loading || (direction === 'prev' ? model.isFirst : model.isLast)) return;
		pending = direction;
		onpage(direction === 'prev' ? model.page - 1 : model.page + 1);
	}

	// After a page change the focus stays on the pressed button, or moves to the other one when the
	// pressed one has just become disabled.
	$effect(() => {
		const snapshot = model;
		if (!pending) return;
		const target = focusAfterPage(pending, snapshot);
		pending = null;
		(target === 'prev' ? prevButton : nextButton)?.focus();
	});
</script>

{#if model.render}
	<nav
		{...rest}
		aria-label={label ?? $t('ui.pagination.label')}
		class="ui-pagination"
		class:compact={density === 'compact'}
		bind:clientWidth={width}
		data-ui="pagination"
	>
		<span class="range" aria-live="polite">{range}</span>
		<div class="controls">
			{#if layout.showSize && onpagesize}
				<span class="size-label" aria-hidden="true">{$t('ui.pagination.rowsPerPage')}</span>
				<div class="size">
					<Select
						id={sizeId}
						label={$t('ui.pagination.rowsPerPage')}
						labelHidden
						density="compact"
						searchable={false}
						nativeOnTouch={false}
						options={sizeOptions}
						value={String(pageSize)}
						onchange={(v) => v && onpagesize(Number(v))}
					/>
				</div>
			{/if}
			<span class="position">{position}</span>
			<button
				bind:this={prevButton}
				type="button"
				class="nav-button"
				aria-label={prevName}
				aria-disabled={model.isFirst || loading || undefined}
				onclick={() => go('prev')}
			>
				<ChevronLeft size={18} aria-hidden="true" />
			</button>
			<button
				bind:this={nextButton}
				type="button"
				class="nav-button"
				aria-label={nextName}
				aria-disabled={model.isLast || loading || undefined}
				onclick={() => go('next')}
			>
				<ChevronRight size={18} aria-hidden="true" />
			</button>
		</div>
	</nav>
{/if}

<style>
	.ui-pagination {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-5);
		box-sizing: border-box;
		min-height: 44px;
		padding: 0 var(--rf-space-5);
		border-top: var(--rf-border-width) solid var(--rf-line);
		background: var(--rf-bg);
		color: var(--rf-secondary);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		font-variant-numeric: tabular-nums;
	}
	.ui-pagination.compact {
		min-height: 36px;
	}
	.range,
	.position {
		white-space: nowrap;
	}
	.controls {
		display: flex;
		align-items: center;
		gap: var(--rf-space-5);
	}
	.size {
		width: 72px;
	}
	.nav-button {
		display: inline-grid;
		place-items: center;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.nav-button:hover:not([aria-disabled='true']) {
		background: var(--rf-hover);
	}
	.nav-button:active:not([aria-disabled='true']) {
		background: var(--rf-pressed);
	}
	.nav-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.nav-button[aria-disabled='true'] {
		color: var(--rf-secondary);
		cursor: not-allowed;
	}
	:global([dir='rtl']) .nav-button :global(svg) {
		transform: scaleX(-1);
	}
	@media (pointer: coarse) {
		.nav-button {
			width: var(--rf-touch-target);
			height: var(--rf-touch-target);
		}
	}
</style>
