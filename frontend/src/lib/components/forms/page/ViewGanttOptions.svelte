<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { selectValue } from '#lib/forms/record-format.js';
	import type { GanttScale } from '#lib/forms/page-types.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		viewDraftGanttDateFieldCandidates,
		viewDraftGanttGroupFieldCandidates,
		viewDraftGanttDependencyFieldCandidates
	} = fp.viewDraftFields;
</script>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.ganttStartField')}</span>

	<select
		value={s.viewDraft.ganttStartField}
		onchange={(event) => {
			const ganttStartField = selectValue(event);

			s.viewDraft = {
				...s.viewDraft,
				ganttStartField,
				ganttEndField:
					s.viewDraft.ganttEndField === ganttStartField ? '' : s.viewDraft.ganttEndField,
				ganttGroupBy: s.viewDraft.ganttGroupBy === ganttStartField ? '' : s.viewDraft.ganttGroupBy,
				ganttDependencyField:
					s.viewDraft.ganttDependencyField === ganttStartField
						? ''
						: s.viewDraft.ganttDependencyField
			};
		}}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftGanttDateFieldCandidates() as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.ganttEndField')}</span>

	<select
		value={s.viewDraft.ganttEndField}
		onchange={(event) => {
			const ganttEndField = selectValue(event);

			s.viewDraft = {
				...s.viewDraft,
				ganttEndField,
				ganttGroupBy: s.viewDraft.ganttGroupBy === ganttEndField ? '' : s.viewDraft.ganttGroupBy,
				ganttDependencyField:
					s.viewDraft.ganttDependencyField === ganttEndField ? '' : s.viewDraft.ganttDependencyField
			};
		}}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.ganttNoEndField')}</option>

		{#each viewDraftGanttDateFieldCandidates().filter((field) => field.key !== s.viewDraft.ganttStartField) as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.ganttGroupField')}</span>

	<select
		value={s.viewDraft.ganttGroupBy}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				ganttGroupBy: selectValue(event),
				ganttDependencyField:
					s.viewDraft.ganttDependencyField === selectValue(event)
						? ''
						: s.viewDraft.ganttDependencyField
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftGanttGroupFieldCandidates() as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.ganttDependencyField')}</span
	>

	<select
		value={s.viewDraft.ganttDependencyField}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				ganttDependencyField: selectValue(event)
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="">{$t('forms.viewNoRule')}</option>

		{#each viewDraftGanttDependencyFieldCandidates() as field (field.key)}
			<option value={field.key}>{fieldLabel(field)}</option>
		{/each}
	</select>
</label>

<label class="space-y-1">
	<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.ganttScale')}</span>

	<select
		value={s.viewDraft.ganttScale}
		onchange={(event) =>
			(s.viewDraft = {
				...s.viewDraft,
				ganttScale: selectValue(event) as GanttScale
			})}
		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
	>
		<option value="day">{$t('forms.ganttScaleDay')}</option>
		<option value="week">{$t('forms.ganttScaleWeek')}</option>
		<option value="month">{$t('forms.ganttScaleMonth')}</option>
	</select>
</label>
