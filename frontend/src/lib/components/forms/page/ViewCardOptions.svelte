<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Plus, X } from '@lucide/svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import type { CardLayoutMode } from '#lib/forms/page-types.js';
	import { isIsoDate, viewCardFieldLimit } from '#lib/forms/view-config.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		addViewDraftCalendarException,
		removeViewDraftCalendarException,
		viewDraftCalendarExceptionItems
	} = fp.viewDraftFields;
</script>

<div class="space-y-2 rounded-card border border-line p-3 lg:col-span-2">
	<div class="grid gap-2 md:grid-cols-[minmax(0,1fr)_180px_auto]">
		{#if s.viewDraft.view_type === 'calendar'}
			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.calendarExceptionRecord')}</span
				>

				<select
					value={s.viewDraft.calendarExceptionRecordId}
					onchange={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							calendarExceptionRecordId: selectValue(event)
						})}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
				>
					<option value="">{$t('forms.viewNoRule')}</option>

					{#each s.viewRecords as record}
						<option value={record.id}>{record.title}</option>
					{/each}
				</select>
			</label>
		{/if}

		{#if s.viewDraft.view_type === 'card'}
			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardFieldLimit')}</span
				>

				<select
					value={String(s.viewDraft.cardFieldLimit)}
					onchange={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							cardFieldLimit: viewCardFieldLimit(selectValue(event))
						})}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
				>
					<option value="3">{$t('forms.cardFieldLimitThree')}</option>
					<option value="6">{$t('forms.cardFieldLimitSix')}</option>
					<option value="9">{$t('forms.cardFieldLimitNine')}</option>
					<option value="99">{$t('forms.cardFieldLimitAll')}</option>
				</select>
			</label>

			<label
				class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
			>
				<input
					type="checkbox"
					class="h-4 w-4 rounded border-input accent-primary"
					checked={s.viewDraft.cardHideEmptyFields}
					data-card-hide-empty-fields-control
					onchange={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							cardHideEmptyFields:
								event.currentTarget instanceof HTMLInputElement
									? event.currentTarget.checked
									: false
						})}
				/>

				<span class="min-w-0 truncate">{$t('forms.cardHideEmptyFields')}</span>
			</label>

			<label
				class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
			>
				<input
					type="checkbox"
					class="h-4 w-4 rounded border-input accent-primary"
					checked={s.viewDraft.cardShowTimestamps}
					data-card-show-timestamps-control
					onchange={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							cardShowTimestamps:
								event.currentTarget instanceof HTMLInputElement
									? event.currentTarget.checked
									: false
						})}
				/>

				<span class="min-w-0 truncate">{$t('forms.cardShowTimestamps')}</span>
			</label>

			<label
				class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
			>
				<input
					type="checkbox"
					class="h-4 w-4 rounded border-input accent-primary"
					checked={s.viewDraft.cardShowRecordId}
					data-card-show-record-id-control
					onchange={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							cardShowRecordId:
								event.currentTarget instanceof HTMLInputElement
									? event.currentTarget.checked
									: false
						})}
				/>

				<span class="min-w-0 truncate">{$t('forms.cardShowRecordId')}</span>
			</label>
		{/if}

		{#if s.viewDraft.view_type === 'calendar'}
			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.calendarExceptionDate')}</span
				>

				<input
					type="date"
					value={s.viewDraft.calendarExceptionDate}
					oninput={(event) =>
						(s.viewDraft = {
							...s.viewDraft,
							calendarExceptionDate: inputValue(event)
						})}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
				/>
			</label>
		{/if}

		<div class="flex items-end">
			<button
				type="button"
				disabled={!s.viewDraft.calendarExceptionRecordId ||
					!isIsoDate(s.viewDraft.calendarExceptionDate)}
				class="inline-flex h-10 items-center gap-2 rounded-control rf-filled bg-primary text-on-primary px-3 text-sm font-medium disabled:bg-surface disabled:text-fg-muted"
				onclick={addViewDraftCalendarException}
			>
				<Plus class="h-4 w-4" aria-hidden="true" />
				{$t('forms.calendarAddException')}
			</button>
		</div>
	</div>

	<div class="space-y-1">
		<div class="text-xs font-medium uppercase text-fg-muted">
			{$t('forms.calendarExceptions')}
		</div>

		{#if viewDraftCalendarExceptionItems().length === 0}
			<p class="text-xs text-fg-muted">
				{$t('forms.calendarNoExceptions')}
			</p>
		{:else}
			<div class="flex flex-wrap gap-2">
				{#each viewDraftCalendarExceptionItems() as exception (`${exception.recordId}:${exception.date}`)}
					<span
						class="inline-flex max-w-full items-center gap-2 rounded-control bg-surface px-2 py-1 text-xs text-fg"
					>
						<span class="truncate">{exception.label} · {exception.date}</span>

						<button
							type="button"
							class="rounded p-0.5 text-fg-muted hover:bg-selected hover:text-fg"
							onclick={() => removeViewDraftCalendarException(exception.recordId, exception.date)}
							aria-label={$t('forms.calendarRemoveException')}
							><X class="h-3 w-3" aria-hidden="true" /></button
						>
					</span>
				{/each}
			</div>
		{/if}
	</div>
</div>

{#if s.viewDraft.view_type === 'card'}
	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardLayout')}</span>

		<select
			value={s.viewDraft.cardLayout}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					cardLayout: selectValue(event) as CardLayoutMode
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="standard">{$t('forms.cardLayoutStandard')}</option>
			<option value="gallery">{$t('forms.cardLayoutGallery')}</option>
			<option value="compact">{$t('forms.cardLayoutCompact')}</option>
			<option value="list">{$t('forms.cardLayoutList')}</option>
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardTitleField')}</span>

		<select
			value={s.viewDraft.cardTitleField}
			onchange={(event) => {
				const cardTitleField = selectValue(event);

				s.viewDraft = {
					...s.viewDraft,
					cardTitleField,
					cardSubtitleField:
						s.viewDraft.cardSubtitleField === cardTitleField ? '' : s.viewDraft.cardSubtitleField,
					cardBadgeField:
						s.viewDraft.cardBadgeField === cardTitleField ? '' : s.viewDraft.cardBadgeField,
					cardFieldKeys:
						cardTitleField && s.viewDraft.cardFieldKeys[0] !== cardTitleField
							? [
									cardTitleField,
									...s.viewDraft.cardFieldKeys.filter((key) => key !== cardTitleField)
								]
							: s.viewDraft.cardFieldKeys
				};
			}}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="">{$t('forms.recordTitle')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardSubtitleField')}</span>

		<select
			value={s.viewDraft.cardSubtitleField}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					cardSubtitleField: selectValue(event),
					cardBadgeField:
						s.viewDraft.cardBadgeField === selectValue(event) ? '' : s.viewDraft.cardBadgeField
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false && field.key !== s.viewDraft.cardTitleField) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardBadgeField')}</span>

		<select
			value={s.viewDraft.cardBadgeField}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					cardBadgeField: selectValue(event)
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false && field.key !== s.viewDraft.cardTitleField && field.key !== s.viewDraft.cardSubtitleField) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>

	<label class="space-y-1">
		<span class="text-xs font-medium uppercase text-fg-muted">{$t('forms.cardCoverField')}</span>

		<select
			value={s.viewDraft.cardCoverField}
			onchange={(event) =>
				(s.viewDraft = {
					...s.viewDraft,
					cardCoverField: selectValue(event)
				})}
			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
		>
			<option value="">{$t('forms.viewNoRule')}</option>

			{#each s.fields.filter((field) => field.visibility?.list !== false && ['image', 'attachment'].includes(field.type ?? 'text')) as field}
				<option value={field.key}>{fieldLabel(field)}</option>
			{/each}
		</select>
	</label>
{/if}
