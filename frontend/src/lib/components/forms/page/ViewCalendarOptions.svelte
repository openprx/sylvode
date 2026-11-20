<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ChevronLeft, ChevronRight } from '@lucide/svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import type {
		ViewCalendarDayScope,
		ViewCalendarLayout,
		ViewCalendarRange,
		ViewCalendarRecurrence
	} from '#lib/forms/page-types.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		shiftViewDraftCalendarFocus,
		viewDraftCalendarEndFieldCandidates,
		viewDraftCalendarResourceFieldCandidates
	} = fp.viewDraftFields;
</script>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarEndField')}</span>

	<select
		value={s.viewDraft.calendarEndField}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarEndField: selectValue(event)
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftCalendarEndFieldCandidates() as field}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted"
		>{$t('forms.calendarResourceField')}</span
	>

	<select
		value={s.viewDraft.calendarResourceBy}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarResourceBy: selectValue(event)
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftCalendarResourceFieldCandidates() as field}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarRange')}</span>

	<select
		value={s.viewDraft.calendarRange}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarRange: selectValue(event) as ViewCalendarRange
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="week">{$t('forms.calendarRangeWeek')}</option>
		<option value="month">{$t('forms.calendarRangeMonth')}</option>
	</select>
</label>

<div class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarFocusDate')}</span>

	<div class="grid grid-cols-[auto_minmax(0,1fr)_auto] gap-1">
		<button
			type="button"
			class="inline-flex h-9 w-9 items-center justify-center rounded-control border border-input text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={() => shiftViewDraftCalendarFocus(-1)}
			aria-label={$t('forms.calendarPreviousRange')}
			title={$t('forms.calendarPreviousRange')}
			data-calendar-focus-shift="-1"><ChevronLeft class="h-4 w-4" aria-hidden="true" /></button
		>

		<input
			type="date"
			value={s.viewDraft.calendarFocusDate}
			aria-label={$t('forms.calendarFocusDate')}
			data-calendar-focus-date-control
			oninput={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					calendarFocusDate: inputValue(event)
				})}
			class="min-w-0 rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		/>

		<button
			type="button"
			class="inline-flex h-9 w-9 items-center justify-center rounded-control border border-input text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
			onclick={() => shiftViewDraftCalendarFocus(1)}
			aria-label={$t('forms.calendarNextRange')}
			title={$t('forms.calendarNextRange')}
			data-calendar-focus-shift="1"><ChevronRight class="h-4 w-4" aria-hidden="true" /></button
		>
	</div>
</div>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarRecurrence')}</span>

	<select
		value={s.viewDraft.calendarRecurrence}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarRecurrence: selectValue(event) as ViewCalendarRecurrence
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="none">{$t('forms.calendarRecurrenceNone')}</option>
		<option value="daily">{$t('forms.calendarRecurrenceDaily')}</option>
		<option value="weekly">{$t('forms.calendarRecurrenceWeekly')}</option>
		<option value="monthly">{$t('forms.calendarRecurrenceMonthly')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarDayScope')}</span>

	<select
		value={s.viewDraft.calendarDayScope}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarDayScope: selectValue(event) as ViewCalendarDayScope
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="all">{$t('forms.calendarDayScopeAll')}</option>
		<option value="workweek">{$t('forms.calendarDayScopeWorkweek')}</option>
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.calendarLayout')}</span>

	<select
		value={s.viewDraft.calendarLayout}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarLayout: selectValue(event) as ViewCalendarLayout
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="grid">{$t('forms.calendarLayoutGrid')}</option>
		<option value="agenda">{$t('forms.calendarLayoutAgenda')}</option>
	</select>
</label>

<label
	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
>
	<input
		type="checkbox"
		class="h-4 w-4 rounded border-input accent-primary"
		checked={s.viewDraft.calendarHideEmptyDays}
		data-calendar-hide-empty-days-control
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				calendarHideEmptyDays:
					event.currentTarget instanceof HTMLInputElement ? event.currentTarget.checked : false
			})}
	/>

	<span class="min-w-0 truncate">{$t('forms.calendarHideEmptyDays')}</span>
</label>
