<script lang="ts">
	import { t } from 'svelte-i18n';
	import ViewPivotOptions from '#lib/components/forms/page/ViewPivotOptions.svelte';
	import ViewGanttOptions from '#lib/components/forms/page/ViewGanttOptions.svelte';
	import ViewCalendarOptions from '#lib/components/forms/page/ViewCalendarOptions.svelte';
	import ViewCardOptions from '#lib/components/forms/page/ViewCardOptions.svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { selectValue } from '#lib/forms/record-format.js';
	import type { ViewSortDirection } from '#lib/forms/page-types.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		viewDraftSwimlaneFieldCandidates
	} = fp.viewDraftFields;
</script>

<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted"
			>{$t('forms.sortField')}</span
		>

		<select
			value={s.viewDraft.sortField}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					sortField: selectValue(event)
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted"
			>{$t('forms.sortDirection')}</span
		>

		<select
			value={s.viewDraft.sortDirection}
			disabled={!s.viewDraft.sortField}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					sortDirection: selectValue(event) as ViewSortDirection
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="asc">{$t('forms.sortAscending')}</option>
			<option value="desc">{$t('forms.sortDescending')}</option>
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted"
			>{$t('forms.groupField')}</span
		>

		<select
			value={s.viewDraft.groupBy}
			onchange={(event) => {
				const groupBy = selectValue(event);

				s.viewDraft = {
					...s.viewDraft,
					groupBy,
					swimlaneBy:
						s.viewDraft.swimlaneBy === groupBy
							? ''
							: s.viewDraft.swimlaneBy,
					wipLimits: {}
				};
			}}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	{#if s.viewDraft.view_type === 'pivot'}
		<ViewPivotOptions />
	{/if}

	{#if s.viewDraft.view_type === 'gantt'}
		<ViewGanttOptions />
	{/if}

	{#if s.viewDraft.view_type === 'kanban'}
		<label class="space-y-1">
			<span class="text-xs font-medium uppercase text-fg-muted"
				>{$t('forms.swimlaneField')}</span
			>

			<select
				value={s.viewDraft.swimlaneBy}
				onchange={(event) =>
					(s.viewDraft = {
						...s.viewDraft,
						swimlaneBy: selectValue(event)
					})}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
			>
				<option value="">{$t('forms.viewNoRule')}</option>

				{#each viewDraftSwimlaneFieldCandidates() as field}
					<option value={field.key}>{fieldLabel(field)}</option>
				{/each}
			</select>
		</label>
	{/if}

	<label class="space-y-1 sm:col-span-2 xl:col-span-1">
		<span class="text-xs font-medium uppercase text-fg-muted"
			>{$t('forms.dateField')}</span
		>

		<select
			value={s.viewDraft.dateField}
			onchange={(event) => {
				const dateField = selectValue(event);

				s.viewDraft = {
					...s.viewDraft,
					dateField,
					calendarEndField:
						s.viewDraft.calendarEndField === dateField
							? ''
							: s.viewDraft.calendarEndField,
					calendarResourceBy:
						s.viewDraft.calendarResourceBy === dateField
							? ''
							: s.viewDraft.calendarResourceBy
				};
			}}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter( (field) => ['date', 'datetime'].includes(field.type ?? 'text') ) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	{#if s.viewDraft.view_type === 'calendar'}
		<ViewCalendarOptions />
	{/if}

	<ViewCardOptions />
</div>
