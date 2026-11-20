<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { t, locale } from 'svelte-i18n';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import ChevronUp from '@lucide/svelte/icons/chevron-up';
	import { uid } from '../foundations/ids';
	import { tabbables, trapTarget } from '../foundations/focus';
	import {
		addMonths,
		bandRole,
		daysInMonth,
		fullDateLabel,
		isDayDisabled,
		monthGrid,
		monthLabel,
		monthName,
		moveFocus,
		moveMonth,
		nearestEnabled,
		parseTimeInput,
		partsOf,
		pickRangeDate,
		stepTime,
		todayIso,
		toIso,
		weekdayLabels,
		yearLabel,
		type DateRange,
		type IsoDate,
		type MoveKey,
		type WeekStart
	} from './calendar';

	interface Props {
		mode?: 'single' | 'range';
		/** `datetime` adds the time field to the footer. */
		kind?: 'date' | 'datetime';
		selected?: IsoDate | null;
		range?: DateRange | null;
		/** The time shown in the footer field (`HH:mm`). */
		time?: string;
		min?: IsoDate;
		max?: IsoDate;
		isDisabled?: (date: IsoDate) => boolean;
		weekStart?: WeekStart;
		/** The line under the footer that says what the time means. */
		zoneLabel?: string;
		/** Single mode: the date that was chosen. */
		onselect?: (date: IsoDate) => void;
		onrange?: (range: DateRange) => void;
		onclear?: () => void;
		/** `commit` is true for Enter in the time field: the caller closes. */
		ontime?: (time: string, commit: boolean) => void;
	}

	let {
		mode = 'single',
		kind = 'date',
		selected = null,
		range = null,
		time = '00:00',
		min,
		max,
		isDisabled,
		weekStart = 1,
		zoneLabel,
		onselect,
		onrange,
		onclear,
		ontime
	}: Props = $props();

	const id = uid('calendar');
	const today = todayIso();
	const bounds = $derived({ min, max, isDisabled });
	const MOVE_KEYS = [
		'ArrowLeft',
		'ArrowRight',
		'ArrowUp',
		'ArrowDown',
		'Home',
		'End',
		'PageUp',
		'PageDown'
	];

	let root = $state<HTMLElement | null>(null);
	let timeEl = $state<HTMLInputElement | null>(null);
	let view = $state<'days' | 'months'>('days');
	let focusDate = $state<IsoDate>(
		untrack(() => nearestEnabled(selected ?? range?.start ?? today, { min, max, isDisabled }))
	);
	let monthYear = $state(untrack(() => partsOf(focusDate).year));
	let monthIndex = $state(untrack(() => partsOf(focusDate).month));
	let pending = $state<IsoDate | null>(null);
	let hover = $state<IsoDate | null>(null);
	let timeText = $state(untrack(() => time));
	let timeInvalid = $state(false);

	const shown = $derived(partsOf(focusDate));
	const grid = $derived(monthGrid(shown.year, shown.month, weekStart));
	const weekdays = $derived(weekdayLabels($locale, weekStart));
	const heading = $derived(
		view === 'days' ? monthLabel(shown.year, shown.month, $locale) : yearLabel(monthYear, $locale)
	);
	const preview = $derived(pending ? (hover ?? focusDate) : null);
	const todayDisabled = $derived(isDayDisabled(today, bounds));

	$effect(() => {
		timeText = time;
		timeInvalid = false;
	});

	async function focusCell(date: IsoDate) {
		await tick();
		root?.querySelector<HTMLElement>(`td[data-date="${date}"]`)?.focus();
	}
	async function focusMonthCell() {
		await tick();
		root?.querySelector<HTMLElement>(`td[data-month="${monthIndex}"]`)?.focus();
	}

	/** Puts the day grid on this date's month and the roving tab stop on the date. */
	function setFocus(date: IsoDate) {
		focusDate = date;
	}

	export function focusGrid() {
		void focusCell(focusDate);
	}
	export function focusTime() {
		void tick().then(() => {
			timeEl?.focus();
			timeEl?.select();
		});
	}
	/** Escape closes the month view first; true when it did (the caller then stays open). */
	export function handleEscape(): boolean {
		if (view !== 'months') return false;
		view = 'days';
		focusGrid();
		return true;
	}

	function choose(date: IsoDate) {
		if (isDayDisabled(date, bounds)) return;
		setFocus(date);
		if (mode === 'single') {
			onselect?.(date);
			return;
		}
		const result = pickRangeDate(pending, date);
		pending = result.pending;
		hover = null;
		if (result.range) onrange?.(result.range);
	}

	function onGridKeydown(event: KeyboardEvent) {
		if (event.ctrlKey || event.metaKey || event.altKey) return;
		if (MOVE_KEYS.includes(event.key)) {
			event.preventDefault();
			const next = moveFocus(focusDate, event.key as MoveKey, event.shiftKey, weekStart);
			setFocus(next);
			void focusCell(next);
		} else if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			choose(focusDate);
		}
	}

	function stepMonth(delta: number) {
		setFocus(addMonths(focusDate, delta));
	}
	function stepYear(delta: number) {
		monthYear += delta;
	}
	function openMonths() {
		monthYear = shown.year;
		monthIndex = shown.month;
		view = 'months';
		void focusMonthCell();
	}
	function chooseMonth(month: number) {
		const day = Math.min(partsOf(focusDate).day, daysInMonth(monthYear, month));
		setFocus(toIso(monthYear, month, day));
		view = 'days';
		focusGrid();
	}
	function onMonthKeydown(event: KeyboardEvent) {
		if (event.ctrlKey || event.metaKey || event.altKey) return;
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			chooseMonth(monthIndex);
			return;
		}
		const next = moveMonth({ year: monthYear, month: monthIndex }, event.key);
		if (!next) return;
		event.preventDefault();
		monthYear = next.year;
		monthIndex = next.month;
		void focusMonthCell();
	}

	function commitTime(commit: boolean) {
		const parsed = parseTimeInput(timeText);
		if (!parsed.ok) {
			timeInvalid = true;
			return;
		}
		timeInvalid = false;
		timeText = parsed.value;
		ontime?.(parsed.value, commit);
	}
	function onTimeKeydown(event: KeyboardEvent) {
		if (event.key === 'Enter') {
			event.preventDefault();
			commitTime(true);
		} else if (
			(event.key === 'ArrowUp' || event.key === 'ArrowDown') &&
			!event.altKey &&
			!event.ctrlKey &&
			!event.metaKey
		) {
			event.preventDefault();
			timeText = stepTime(timeText, event.key, event.shiftKey);
			commitTime(false);
		}
	}

	function onRootKeydown(event: KeyboardEvent) {
		if (event.key !== 'Tab' || !root) return;
		const target = trapTarget(
			tabbables(root),
			document.activeElement as HTMLElement | null,
			event.shiftKey,
			(item) => item !== null && !!root?.contains(item)
		);
		if (target) {
			event.preventDefault();
			target.focus();
		}
	}
</script>

<!-- Tab cycles inside the panel: previous, heading, next, grid (one stop), time, Clear, Today. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="panel" bind:this={root} onkeydown={onRootKeydown} data-ui="calendar">
	<div class="header">
		<button
			type="button"
			class="nav"
			aria-label={$t(view === 'days' ? 'ui.date.previousMonth' : 'ui.date.previousYear')}
			onclick={() => (view === 'days' ? stepMonth(-1) : stepYear(-1))}
		>
			<ChevronLeft size={16} aria-hidden="true" />
		</button>
		<button
			type="button"
			class="heading"
			aria-expanded={view === 'months'}
			onclick={() => (view === 'days' ? openMonths() : ((view = 'days'), focusGrid()))}
		>
			<span>{heading}</span>
			{#if view === 'days'}<ChevronDown size={14} aria-hidden="true" />{:else}<ChevronUp
					size={14}
					aria-hidden="true"
				/>{/if}
		</button>
		<button
			type="button"
			class="nav"
			aria-label={$t(view === 'days' ? 'ui.date.nextMonth' : 'ui.date.nextYear')}
			onclick={() => (view === 'days' ? stepMonth(1) : stepYear(1))}
		>
			<ChevronRight size={16} aria-hidden="true" />
		</button>
	</div>
	<!-- Repeats the month and year to assistive technology when the view changes. -->
	<h2 class="sr-only" id="{id}-heading" aria-live="polite">{heading}</h2>

	{#if view === 'days'}
		<table class="grid" role="grid" aria-labelledby="{id}-heading" onkeydown={onGridKeydown}>
			<thead>
				<tr>
					{#each weekdays as day (day.full)}
						<th scope="col" abbr={day.full}>{day.short}</th>
					{/each}
				</tr>
			</thead>
			<tbody>
				{#each grid as row, rowIndex (rowIndex)}
					<tr>
						{#each row as date, col (col)}
							{#if date}
								{@const band = bandRole(date, range, pending, preview)}
								{@const isEnd =
									band.role === 'start' || band.role === 'end' || band.role === 'single'}
								{@const marked = mode === 'single' ? date === selected : isEnd}
								{@const off = isDayDisabled(date, bounds)}
								<td
									role="gridcell"
									class="cell"
									class:sel={marked}
									class:today={date === today}
									class:band={band.role !== null && !band.preview}
									class:preview={band.role !== null && band.preview}
									class:start={band.role === 'start' || band.role === 'single'}
									class:end={band.role === 'end' || band.role === 'single'}
									tabindex={date === focusDate ? 0 : -1}
									aria-selected={mode === 'single'
										? date === selected
										: band.role !== null && !band.preview}
									aria-current={date === today ? 'date' : undefined}
									aria-disabled={off || undefined}
									data-date={date}
									onclick={() => choose(date)}
									onfocus={() => (focusDate = date)}
									onpointerenter={() => (hover = date)}
									onpointerleave={() => (hover = null)}
								>
									<span class="num" aria-hidden="true">{partsOf(date).day}</span>
									<span class="sr-only">
										{fullDateLabel(
											date,
											$locale
										)}{#if mode === 'range' && (band.role === 'start' || band.role === 'single')}, {$t(
												'ui.date.rangeStart'
											)}{/if}{#if mode === 'range' && (band.role === 'end' || band.role === 'single') && !band.preview},
											{$t('ui.date.rangeEnd')}{/if}
									</span>
								</td>
							{:else}
								<td role="gridcell" class="cell empty"></td>
							{/if}
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<table class="months" role="grid" aria-labelledby="{id}-heading" onkeydown={onMonthKeydown}>
			<tbody>
				{#each [0, 1, 2, 3] as r (r)}
					<tr>
						{#each [1, 2, 3] as c (c)}
							{@const m = r * 3 + c}
							<td
								role="gridcell"
								class="month"
								class:sel={monthYear === shown.year && m === shown.month}
								tabindex={m === monthIndex ? 0 : -1}
								aria-selected={monthYear === shown.year && m === shown.month}
								data-month={m}
								onclick={() => chooseMonth(m)}
								onfocus={() => (monthIndex = m)}
							>
								{monthName(m, $locale)}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}

	<div class="footer">
		{#if kind === 'datetime'}
			<input
				bind:this={timeEl}
				bind:value={timeText}
				class="time"
				class:invalid={timeInvalid}
				type="text"
				autocomplete="off"
				spellcheck="false"
				aria-label={$t('ui.date.time')}
				aria-invalid={timeInvalid || undefined}
				title={timeInvalid ? $t('ui.date.error.time') : undefined}
				placeholder="HH:mm"
				oninput={() => (timeInvalid = false)}
				onkeydown={onTimeKeydown}
				onblur={() => timeText !== time && commitTime(false)}
			/>
		{/if}
		<span class="actions">
			<button type="button" class="text-button" onclick={() => onclear?.()}
				>{$t('ui.date.clear')}</button
			>
			<button
				type="button"
				class="text-button"
				aria-disabled={todayDisabled || undefined}
				onclick={() => choose(today)}
			>
				{$t('ui.date.today')}
			</button>
		</span>
	</div>
	{#if kind === 'datetime' && zoneLabel}<p class="zone">{zoneLabel}</p>{/if}
</div>

<style>
	.panel {
		--cell: 32px;
		box-sizing: border-box;
		padding: var(--rf-space-5);
		width: calc(7 * var(--cell) + 2 * var(--rf-space-5));
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
	}
	.header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: var(--cell);
		margin-bottom: var(--rf-space-4);
	}
	.nav,
	.heading,
	.text-button {
		border: 0;
		background: transparent;
		font: inherit;
		cursor: pointer;
	}
	.nav {
		display: inline-grid;
		place-items: center;
		width: var(--cell);
		height: var(--cell);
		padding: 0;
		border-radius: var(--rf-radius-button);
		color: var(--rf-secondary);
	}
	.heading {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		height: var(--cell);
		padding: 0 var(--rf-space-4);
		border-radius: var(--rf-radius-button);
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
		white-space: nowrap;
	}
	.heading :global(svg) {
		color: var(--rf-control);
	}
	.nav:hover,
	.heading:hover,
	.text-button:hover {
		background: var(--rf-hover);
	}
	.nav:active,
	.heading:active,
	.text-button:active {
		background: var(--rf-pressed);
	}
	.nav:focus-visible,
	.heading:focus-visible,
	.text-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.grid,
	.months {
		border-collapse: collapse;
		table-layout: fixed;
		width: 100%;
	}
	th {
		width: var(--cell);
		height: 24px;
		padding: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		font-weight: var(--rf-weight-regular);
		text-align: center;
	}
	.cell {
		position: relative;
		width: var(--cell);
		height: var(--cell);
		padding: 0;
		border-radius: var(--rf-radius-button);
		text-align: center;
		font-variant-numeric: tabular-nums;
		cursor: pointer;
	}
	.cell.empty {
		cursor: default;
	}
	.cell:not(.empty):hover {
		background: var(--rf-hover);
	}
	.cell:not(.empty):active {
		background: var(--rf-pressed);
	}
	.cell:focus-visible,
	.month:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -2px;
	}
	.num {
		display: inline-grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border-radius: var(--rf-radius-xs);
		font-size: var(--rf-size-secondary);
	}
	.today .num {
		background: var(--rf-primary);
		color: var(--rf-on-primary);
	}
	.cell[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	.cell[aria-disabled='true']:hover {
		background: transparent;
	}
	.today[aria-disabled='true'] .num {
		color: var(--rf-on-primary);
	}
	/* Selected: fill, weight 600 and a 2px bar (the marker does not rely on the fill alone). */
	.sel {
		background: var(--rf-selected);
		font-weight: var(--rf-weight-semibold);
	}
	.sel:not(.empty):hover {
		background: var(--rf-selected);
	}
	.sel::after {
		content: '';
		position: absolute;
		inset-block-end: 2px;
		inset-inline-start: 50%;
		width: 16px;
		height: 2px;
		margin-inline-start: -8px;
		border-radius: 2px;
		background: var(--rf-primary);
	}
	/* Range band: continuous, radius only on the outer sides of the ends. */
	.band,
	.band:not(.empty):hover {
		background: var(--rf-line-strong);
		border-radius: 0;
	}
	.band.start {
		border-start-start-radius: var(--rf-radius-xs);
		border-end-start-radius: var(--rf-radius-xs);
	}
	.band.end {
		border-start-end-radius: var(--rf-radius-xs);
		border-end-end-radius: var(--rf-radius-xs);
	}
	.preview,
	.preview:not(.empty):hover {
		background: var(--rf-selected);
		border-radius: 0;
	}
	.month {
		position: relative;
		width: 72px;
		height: var(--cell);
		padding: 0;
		border-radius: var(--rf-radius-button);
		text-align: center;
		cursor: pointer;
	}
	.month:hover {
		background: var(--rf-hover);
	}
	.month.sel:hover {
		background: var(--rf-selected);
	}
	.months {
		border-collapse: separate;
		border-spacing: var(--rf-space-3) var(--rf-space-2);
		margin: calc(-1 * var(--rf-space-2)) calc(-1 * var(--rf-space-3));
		width: calc(100% + 2 * var(--rf-space-3));
	}
	.footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
		margin-top: var(--rf-space-4);
		padding-top: var(--rf-space-4);
		border-top: var(--rf-border-width) solid var(--rf-line);
	}
	.actions {
		display: inline-flex;
		gap: var(--rf-space-2);
		margin-inline-start: auto;
	}
	.text-button {
		height: 24px;
		padding: 0 var(--rf-space-4);
		border-radius: var(--rf-radius-button);
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-medium);
	}
	.text-button[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	.time {
		box-sizing: border-box;
		width: 96px;
		height: 32px;
		padding: 0 var(--rf-space-4);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-body);
		font-variant-numeric: tabular-nums;
	}
	.time:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.time.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
	.zone {
		margin: var(--rf-space-4) 0 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
	}
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	@media (pointer: coarse) {
		.panel {
			--cell: var(--rf-touch-target);
		}
		.month {
			width: auto;
		}
		.text-button {
			height: var(--rf-touch-target);
		}
	}
</style>
