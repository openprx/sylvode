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
	import {
		joinDateTime,
		nearestEnabled,
		splitDateTime,
		todayIso,
		type IsoDate,
		type WeekStart
	} from './calendar';
	import { displayValue, parseField } from './date-field';

	interface Props {
		kind?: 'date' | 'datetime';
		/** `YYYY-MM-DD` or `YYYY-MM-DDTHH:mm`, or `null`. */
		value: string | null;
		onchange: (value: string | null) => void;
		label: string;
		helper?: string;
		/** Replaces the field's own parse and range messages. */
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
		/** Time given to a chosen date that has none; default `00:00`. */
		defaultTime?: string;
		/** What the time means ("No time zone: saved as entered."), date-time only. */
		zoneLabel?: string;
		density?: 'comfortable' | 'compact';
		/** Reports the field's own message (or `null`) when it changes, for form wiring. */
		onerror?: (message: string | null) => void;
		id?: string;
		name?: string;
	}

	let {
		kind = 'date',
		value,
		onchange,
		label,
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
		defaultTime = '00:00',
		zoneLabel,
		density = 'comfortable',
		onerror,
		id = uid('date'),
		name
	}: Props = $props();

	const popupId = $derived(`${id}-popup`);
	let open = $state(false);
	let text = $state(untrack(() => displayValue(kind, value)));
	let lastValue: string | null = untrack(() => value);
	let armed = $state(false);
	let box = $state<HTMLElement | null>(null);
	let input = $state<HTMLInputElement | null>(null);
	let panel = $state<{ focusGrid(): void; focusTime(): void; handleEscape(): boolean } | null>(
		null
	);

	const bounds = $derived({ min, max, isDisabled });
	const parts = $derived(splitDateTime(value));
	const parsed = $derived(parseField(kind, text, bounds, value, defaultTime));
	// First time: no error while typing. Once shown it follows every input.
	const ownError = $derived(
		armed && parsed.state === 'error'
			? $t(`ui.date.error.${parsed.key}`, { values: { min: min ?? '', max: max ?? '' } })
			: null
	);
	const shownError = $derived(error ?? ownError ?? undefined);

	// A value set from outside replaces what is typed.
	$effect(() => {
		if (value !== lastValue) {
			lastValue = value;
			text = displayValue(kind, value);
			armed = false;
		}
	});
	$effect(() => onerror?.(ownError));

	function emit(next: string | null) {
		lastValue = next;
		onchange(next);
	}

	function commitText() {
		const result = parsed;
		if (result.state === 'empty') {
			armed = false;
			if (value !== null) emit(null);
		} else if (result.state === 'ok') {
			text = displayValue(kind, result.value);
			if (result.value !== value) emit(result.value);
		} else {
			armed = true;
		}
	}

	function openPanel() {
		if (disabled || readonly || open) return;
		open = true;
	}
	function closePanel() {
		if (!open) return;
		open = false;
		input?.focus();
	}
	function onPopoverClose() {
		if (panel?.handleEscape()) return;
		closePanel();
	}

	function onInputKeydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown' && !event.ctrlKey && !event.metaKey) {
			event.preventDefault();
			openPanel();
		} else if (event.key === 'Enter') {
			commitText();
		}
	}

	function setDate(date: IsoDate) {
		if (kind === 'date') {
			text = date;
			emit(date);
			closePanel();
			return;
		}
		const next = joinDateTime(date, parts?.time ?? defaultTime);
		text = displayValue(kind, next);
		emit(next);
		panel?.focusTime();
	}
	function setTime(time: string, commit: boolean) {
		const date = parts?.date ?? nearestEnabled(todayIso(), bounds);
		const next = joinDateTime(date, time);
		text = displayValue(kind, next);
		if (next !== value) emit(next);
		if (commit) closePanel();
	}
	function clear() {
		text = '';
		armed = false;
		if (value !== null) emit(null);
		closePanel();
	}

	const title = $derived($t(kind === 'datetime' ? 'ui.date.chooseDateTime' : 'ui.date.chooseDate'));
</script>

<Field
	{id}
	{label}
	requirement={required ? 'required' : 'optional'}
	{marking}
	helper={disabled && disabledReason ? disabledReason : helper}
	error={shownError}
>
	{#snippet children({ describedBy, invalid, required: isRequired })}
		<DateInput
			{id}
			bind:text
			bind:box
			bind:input
			{name}
			placeholder={$t(kind === 'datetime' ? 'ui.date.placeholderTime' : 'ui.date.placeholder')}
			{disabled}
			{readonly}
			{invalid}
			required={isRequired}
			compact={density === 'compact'}
			{describedBy}
			{popupId}
			expanded={open}
			onkeydown={onInputKeydown}
			onblur={commitText}
		>
			{#snippet trailing()}
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
			{/snippet}
		</DateInput>
	{/snippet}
</Field>

<Popover
	{open}
	anchor={box}
	onclose={onPopoverClose}
	label={title}
	sheetTitle={label}
	id={popupId}
	padding="none"
	width={248}
	initialFocus="td[tabindex='0']"
>
	<CalendarPanel
		bind:this={panel}
		{kind}
		selected={parts?.date ?? null}
		time={parts?.time ?? defaultTime}
		{min}
		{max}
		{isDisabled}
		{weekStart}
		{zoneLabel}
		onselect={setDate}
		ontime={setTime}
		onclear={clear}
	/>
</Popover>

<style>
	.calendar-button {
		display: inline-grid;
		place-items: center;
		flex: none;
		width: 24px;
		height: 24px;
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
		color: var(--rf-secondary);
		cursor: not-allowed;
	}
	@media (pointer: coarse) {
		.calendar-button {
			width: 44px;
			height: 36px;
		}
	}
</style>
