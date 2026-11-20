<script lang="ts">
	import { untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import Calendar from '@lucide/svelte/icons/calendar';
	import Field from '../form/Field.svelte';
	import type { Marking } from '../form/field-state';
	import Popover from '../popover/Popover.svelte';
	import { uid } from '../foundations/ids';
	import CalendarPanel from './CalendarPanel.svelte';
	import DateInput from './DateInput.svelte';
	import type { DateRange, IsoDate, WeekStart } from './calendar';
	import { parseRange } from './date-field';

	interface Props {
		value: DateRange | null;
		onchange: (value: DateRange | null) => void;
		/** Names the group of two boxes. */
		label: string;
		startLabel?: string;
		endLabel?: string;
		helper?: string;
		error?: string;
		required?: boolean;
		marking?: Marking;
		min?: IsoDate;
		max?: IsoDate;
		isDisabled?: (date: IsoDate) => boolean;
		disabled?: boolean;
		disabledReason?: string;
		readonly?: boolean;
		weekStart?: WeekStart;
		density?: 'comfortable' | 'compact';
		id?: string;
	}

	let {
		value,
		onchange,
		label,
		startLabel,
		endLabel,
		helper,
		error,
		required = false,
		marking,
		min,
		max,
		isDisabled,
		disabled = false,
		disabledReason,
		readonly = false,
		weekStart = 1,
		density = 'comfortable',
		id = uid('range')
	}: Props = $props();

	const endId = $derived(`${id}-end`);
	const popupId = $derived(`${id}-popup`);
	let open = $state(false);
	let startText = $state(untrack(() => value?.start ?? ''));
	let endText = $state(untrack(() => value?.end ?? ''));
	let lastValue: DateRange | null = untrack(() => value);
	let armed = $state(false);
	let group = $state<HTMLElement | null>(null);
	let startInput = $state<HTMLInputElement | null>(null);
	let panel = $state<{ focusGrid(): void; handleEscape(): boolean } | null>(null);

	const bounds = $derived({ min, max, isDisabled });
	const parsed = $derived(parseRange(startText, endText, bounds));
	const ownError = $derived.by(() => {
		if (!armed) return null;
		const failing =
			parsed.start.state === 'error'
				? parsed.start
				: parsed.end.state === 'error'
					? parsed.end
					: null;
		if (failing && failing.state === 'error')
			return $t(`ui.date.error.${failing.key}`, { values: { min: min ?? '', max: max ?? '' } });
		return parsed.order ? $t('ui.date.error.order') : null;
	});
	const shownError = $derived(error ?? ownError ?? undefined);
	const startInvalid = $derived(armed && (parsed.start.state === 'error' || parsed.order));
	const endInvalid = $derived(armed && (parsed.end.state === 'error' || parsed.order));
	const title = $derived($t('ui.date.chooseDates'));

	// A range set from outside replaces what is typed.
	$effect(() => {
		if (value?.start !== lastValue?.start || value?.end !== lastValue?.end) {
			startText = value?.start ?? '';
			endText = value?.end ?? '';
			armed = false;
		}
		lastValue = value;
	});

	function emit(next: DateRange | null) {
		lastValue = next;
		onchange(next);
	}

	function commitText() {
		if (parsed.start.state === 'empty' && parsed.end.state === 'empty') {
			armed = false;
			if (value !== null) emit(null);
		} else if (parsed.range) {
			startText = parsed.range.start;
			endText = parsed.range.end;
			if (parsed.range.start !== value?.start || parsed.range.end !== value?.end)
				emit(parsed.range);
		} else if (parsed.start.state === 'error' || parsed.end.state === 'error' || parsed.order) {
			armed = true;
		}
		// One end filled and the other empty is a draft: nothing is emitted yet.
	}

	function openPanel() {
		if (disabled || readonly || open) return;
		open = true;
	}
	function closePanel() {
		if (!open) return;
		open = false;
		startInput?.focus();
	}
	function onPopoverClose() {
		if (panel?.handleEscape()) return;
		closePanel();
	}
	function onKeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown' && !event.ctrlKey && !event.metaKey) {
			event.preventDefault();
			openPanel();
		} else if (event.key === 'Enter') {
			commitText();
		}
	}
	function onrange(range: DateRange) {
		startText = range.start;
		endText = range.end;
		armed = false;
		emit(range);
		closePanel();
	}
	function clear() {
		startText = '';
		endText = '';
		armed = false;
		if (value !== null) emit(null);
		closePanel();
	}
</script>

<Field
	{id}
	{label}
	requirement={required ? 'required' : 'optional'}
	{marking}
	helper={disabled && disabledReason ? disabledReason : helper}
	error={shownError}
>
	{#snippet children({ describedBy, required: isRequired })}
		<div bind:this={group} class="group" role="group" aria-labelledby="{id}-label">
			<DateInput
				{id}
				bind:text={startText}
				bind:input={startInput}
				label={startLabel ?? $t('ui.date.start')}
				placeholder={$t('ui.date.placeholder')}
				{disabled}
				{readonly}
				invalid={startInvalid}
				required={isRequired}
				compact={density === 'compact'}
				width={140}
				{describedBy}
				{popupId}
				expanded={open}
				onkeydown={onKeydown}
				onblur={commitText}
			/>
			<span class="dash" aria-hidden="true">–</span>
			<DateInput
				id={endId}
				bind:text={endText}
				label={endLabel ?? $t('ui.date.end')}
				placeholder={$t('ui.date.placeholder')}
				{disabled}
				{readonly}
				invalid={endInvalid}
				required={isRequired}
				compact={density === 'compact'}
				width={140}
				{describedBy}
				{popupId}
				expanded={open}
				onkeydown={onKeydown}
				onblur={commitText}
			/>
			{#if !readonly}
				<button
					type="button"
					class="calendar-button"
					{disabled}
					aria-label={title}
					aria-haspopup="dialog"
					aria-expanded={open}
					onclick={() => (open ? closePanel() : openPanel())}
				>
					<Calendar size={16} aria-hidden="true" />
				</button>
			{/if}
		</div>
	{/snippet}
</Field>

<Popover
	{open}
	anchor={group}
	onclose={onPopoverClose}
	label={title}
	id={popupId}
	padding="none"
	width={248}
	initialFocus="td[tabindex='0']"
>
	<CalendarPanel
		bind:this={panel}
		mode="range"
		range={value}
		{min}
		{max}
		{isDisabled}
		{weekStart}
		{onrange}
		onclear={clear}
	/>
</Popover>

<style>
	.group {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		min-width: 0;
	}
	.dash {
		color: var(--rf-secondary);
	}
	.calendar-button {
		display: inline-grid;
		place-items: center;
		flex: none;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
	}
	.calendar-button:hover:not(:disabled) {
		background: var(--rf-hover);
	}
	.calendar-button:active:not(:disabled) {
		background: var(--rf-pressed);
	}
	.calendar-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.calendar-button:disabled {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	@media (pointer: coarse) {
		.calendar-button {
			width: var(--rf-touch-target);
			height: var(--rf-touch-target);
		}
	}
</style>
