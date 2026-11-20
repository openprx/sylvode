<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ChevronLeft, ChevronRight, Copy, Filter, Trash2 } from '@lucide/svelte';
	import {
		calendarAgendaGroups,
		calendarAgendaRecordCount,
		calendarDateInputValue,
		calendarRecordIsoDate,
		calendarScheduleKey
	} from '#lib/forms/calendar-dates.js';
	import {
		fieldHasConditionalHidden,
		fieldHasConditionalReadOnly,
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { inputValue, recordDisplay } from '#lib/forms/record-format.js';
	import { viewCalendarDayScope, viewCalendarRange } from '#lib/forms/view-config.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		applyCalendarBucketFilters,
		calendarBucketFilterKey,
		calendarResourceRows,
		canApplyCalendarBucketFilters,
		canShiftCalendarOccurrence,
		copyCalendarSchedule,
		handleCalendarDragEnd,
		handleCalendarDragOver,
		handleCalendarDragStart,
		handleCalendarDrop,
		shiftCalendarOccurrence,
		updateCalendarRecordDate,
		updateCalendarRecordEndDate,
		updateCalendarSeriesDate,
		viewCalendarFocusDate
	} = fp.viewCalendar;
	const { handleDeleteRecord } = fp.records;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const { selectRecord } = fp.recordPanel;
</script>

<div class="overflow-x-auto p-4" data-view-renderer="calendar">
	<div class="mb-3 flex flex-wrap items-center gap-1.5 text-xs text-fg-muted">
		{$t('forms.viewDateBy', {
			values: {
				field: s.calendarDateField
					? fieldLabel(s.calendarDateField)
					: $t('forms.updated')
			}
		})}

		{#if s.calendarDateField && memberFieldHidden(s.calendarDateField)}
			<span
				class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
				data-calendar-date-field-permission-hidden={s.calendarDateField.key}
				>{$t('forms.fieldHiddenBadge')}</span
			>
		{/if}

		{#if s.calendarDateField && memberFieldLocked(s.calendarDateField)}
			<span
				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
				data-calendar-date-field-permission-locked={s.calendarDateField.key}
				>{$t('forms.fieldLockedBadge')}</span
			>
		{/if}

		{#if s.calendarDateField && fieldHasConditionalHidden(s.calendarDateField)}
			<span
				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
				data-calendar-date-field-conditional-hidden={s.calendarDateField.key}
				>{$t('forms.fieldConditionHiddenBadge')}</span
			>
		{/if}

		{#if s.calendarDateField && fieldHasConditionalReadOnly(s.calendarDateField)}
			<span
				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
				data-calendar-date-field-conditional-readonly={s.calendarDateField.key}
				>{$t('forms.fieldConditionReadOnlyBadge')}</span
			>
		{/if}
	</div>

	<div
		class="min-w-[980px] space-y-4"
		data-calendar-range={viewCalendarRange(
			s.selectedView?.config?.calendar_range
		)}
		data-calendar-day-scope={viewCalendarDayScope(
			s.selectedView?.config?.calendar_day_scope
		)}
		data-calendar-layout={s.selectedCalendarLayout}
		data-calendar-hide-empty-days={s.selectedCalendarHideEmptyDays}
		data-calendar-focus-date={viewCalendarFocusDate()}
	>
		{#each calendarResourceRows() as resource}
			<section
				class={s.calendarResourceField
					? 'rounded-card border border-line bg-surface p-3'
					: ''}
				data-calendar-resource={s.calendarResourceField
					? resource.label
					: undefined}
				data-record-count={resource.records.length}
			>
				{#if s.calendarResourceField}
					<div class="mb-3 flex items-center justify-between gap-3">
						<div class="min-w-0">
							<div
								class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted"
							>
								{$t('forms.viewCalendarResourceBy', {
									values: { field: fieldLabel(s.calendarResourceField) }
								})}

								{#if memberFieldHidden(s.calendarResourceField)}
									<span
										class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
										data-calendar-resource-field-permission-hidden={s
											.calendarResourceField.key}
										>{$t('forms.fieldHiddenBadge')}</span
									>
								{/if}

								{#if memberFieldLocked(s.calendarResourceField)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-calendar-resource-field-permission-locked={s
											.calendarResourceField.key}
										>{$t('forms.fieldLockedBadge')}</span
									>
								{/if}

								{#if fieldHasConditionalHidden(s.calendarResourceField)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-calendar-resource-field-conditional-hidden={s
											.calendarResourceField.key}
										>{$t('forms.fieldConditionHiddenBadge')}</span
									>
								{/if}

								{#if fieldHasConditionalReadOnly(s.calendarResourceField)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-calendar-resource-field-conditional-readonly={s
											.calendarResourceField.key}
										>{$t('forms.fieldConditionReadOnlyBadge')}</span
									>
								{/if}
							</div>

							<h4 class="truncate text-sm font-semibold text-fg">
								{resource.label}
							</h4>
						</div>

						<span class="rounded bg-raised px-2 py-0.5 text-xs text-fg-muted"
							>{$t('forms.groupRecordCount', {
								values: { count: resource.records.length }
							})}</span
						>
					</div>
				{/if}

				{#if s.selectedCalendarLayout === 'agenda'}
					<div
						class="space-y-3"
						data-calendar-agenda-resource={s.calendarResourceField
							? resource.label
							: '__all__'}
						data-record-count={calendarAgendaRecordCount(resource)}
					>
						{#each calendarAgendaGroups(resource) as group}
							<section
								class="rounded-card border border-line bg-raised"
								data-calendar-agenda-date={group.date}
								data-record-count={group.records.length}
							>
								<div
									class="flex items-center justify-between gap-2 border-b border-line px-3 py-2"
								>
									<h4 class="truncate text-sm font-semibold text-fg">
										{group.label}
									</h4>

									<div class="flex shrink-0 items-center gap-1">
										<button
											type="button"
											class="rounded-control p-1.5 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
											onclick={() =>
												applyCalendarBucketFilters(group.date, resource)}
											disabled={!canApplyCalendarBucketFilters(
												group.date,
												resource
											)}
											aria-label={$t('forms.applyCalendarBucketFilters')}
											title={$t('forms.applyCalendarBucketFilters')}
											data-calendar-agenda-apply-filters={`${group.date}:${s.calendarResourceField ? resource.key : '__all__'}`}
											data-calendar-filters-applied={s.appliedCalendarFilterKey ===
											calendarBucketFilterKey(group.date, resource)
												? 'true'
												: undefined}
											><Filter class="h-3.5 w-3.5" aria-hidden="true" /></button
										>

										<span
											class="rounded bg-nav px-2 py-0.5 text-xs text-fg-muted"
											>{$t('forms.groupRecordCount', {
												values: { count: group.records.length }
											})}</span
										>
									</div>
								</div>

								<div class="divide-y divide-line">
									{#each group.records as occurrence (occurrence.key)}
										{@const record = occurrence.record}

										<article
											class="grid gap-3 px-3 py-3 sm:grid-cols-[8rem_minmax(0,1fr)_auto]"
											data-calendar-agenda-occurrence={occurrence.spanStart
												? `${record.id}:${occurrence.startDate}`
												: occurrence.key}
											data-calendar-card={record.id}
											data-calendar-recurring={occurrence.recurring}
											data-calendar-start={occurrence.startDate}
											data-calendar-end={occurrence.endDate}
											data-calendar-span-days={occurrence.spanDays}
											data-calendar-schedule-copied={s.copiedCalendarScheduleKey ===
											calendarScheduleKey(occurrence)
												? 'true'
												: undefined}
											data-calendar-shifted={s.shiftedCalendarScheduleKey ===
											calendarScheduleKey(occurrence)
												? 'true'
												: undefined}
										>
											<div class="min-w-0 text-xs font-medium text-fg-muted">
												<div class="truncate">{occurrence.startDate}</div>

												{#if occurrence.spanDays > 1}
													<div class="mt-1 truncate text-success">
														{$t('forms.calendarDateSpan', {
															values: {
																start: occurrence.startDate,
																end: occurrence.endDate
															}
														})}
													</div>
												{/if}
											</div>

											<button
												type="button"
												class="min-w-0 text-left"
												onclick={() => selectRecord(record)}
												aria-label={`${$t('forms.openRecord')}: ${record.title}`}
											>
												<span
													class="block truncate text-sm font-semibold text-fg"
													>{record.title}</span
												>

												<span
													class="mt-0.5 block truncate text-xs text-fg-muted"
												>
													{s.visibleFields[0]
														? recordDisplay(record, s.visibleFields[0])
														: record.id.slice(0, 8)}
												</span>

												{#if s.visibleFields[0] && recordFieldHiddenByCondition(s.visibleFields[0], record)}
													<span
														class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
														data-calendar-summary-conditional-hidden={`${record.id}:${s.visibleFields[0].key}`}
														>{$t('forms.fieldConditionHiddenBadge')}</span
													>
												{/if}

												{#if s.visibleFields[0] && recordFieldReadOnlyByCondition(s.visibleFields[0], record)}
													<span
														class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
														data-calendar-summary-conditional-readonly={`${record.id}:${s.visibleFields[0].key}`}
														>{$t('forms.fieldConditionReadOnlyBadge')}</span
													>
												{/if}

												{#if occurrence.recurring}
													<span
														class="mt-2 inline-flex rounded bg-surface px-2 py-0.5 text-xs font-medium text-fg-muted"
														>{$t('forms.calendarRecurringOccurrence')}</span
													>
												{/if}
											</button>

											<div class="flex items-center justify-end gap-1">
												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
													onclick={() =>
														shiftCalendarOccurrence(occurrence, -1)}
													disabled={s.updatingCalendarRecordId === record.id ||
														!canShiftCalendarOccurrence(occurrence)}
													aria-label={$t('forms.calendarShiftPreviousDay')}
													title={$t('forms.calendarShiftPreviousDay')}
													data-calendar-shift-previous={calendarScheduleKey(
														occurrence
													)}
													><ChevronLeft
														class="h-3.5 w-3.5"
														aria-hidden="true"
													/></button
												>

												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
													onclick={() => shiftCalendarOccurrence(occurrence, 1)}
													disabled={s.updatingCalendarRecordId === record.id ||
														!canShiftCalendarOccurrence(occurrence)}
													aria-label={$t('forms.calendarShiftNextDay')}
													title={$t('forms.calendarShiftNextDay')}
													data-calendar-shift-next={calendarScheduleKey(
														occurrence
													)}
													><ChevronRight
														class="h-3.5 w-3.5"
														aria-hidden="true"
													/></button
												>

												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control text-fg-muted hover:bg-surface hover:text-fg"
													onclick={() => copyCalendarSchedule(occurrence)}
													aria-label={$t('forms.copyCalendarSchedule')}
													title={$t('forms.copyCalendarSchedule')}
													data-calendar-copy-schedule={calendarScheduleKey(
														occurrence
													)}
													><Copy
														class="h-3.5 w-3.5"
														aria-hidden="true"
													/></button
												>

												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
													disabled={s.deletingRecordId === record.id}
													onclick={() => handleDeleteRecord(record)}
													aria-label={$t('forms.deleteRecord')}
													><Trash2
														class="h-3.5 w-3.5"
														aria-hidden="true"
													/></button
												>
											</div>
										</article>
									{/each}
								</div>
							</section>
						{/each}
					</div>
				{:else}
					<div
						class={`grid gap-3 ${
							viewCalendarRange(s.selectedView?.config?.calendar_range) ===
							'month'
								? 'grid-cols-2 md:grid-cols-4 xl:grid-cols-7'
								: 'lg:grid-cols-7'
						}`}
					>
						{#each resource.groups as group}
							<section
								class="min-h-48 rounded-card border border-line bg-nav"
								data-calendar-date={group.date}
								data-record-count={group.records.length}
								role="list"
								ondragover={handleCalendarDragOver}
								ondrop={(event) => handleCalendarDrop(event, group.date)}
							>
								<div
									class="flex items-center justify-between gap-2 border-b border-line px-3 py-2"
								>
									<h4 class="truncate text-sm font-semibold text-fg">
										{group.label}
									</h4>

									<div class="flex shrink-0 items-center gap-1">
										<button
											type="button"
											class="rounded-control p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
											onclick={() =>
												applyCalendarBucketFilters(group.date, resource)}
											disabled={!canApplyCalendarBucketFilters(
												group.date,
												resource
											)}
											aria-label={$t('forms.applyCalendarBucketFilters')}
											title={$t('forms.applyCalendarBucketFilters')}
											data-calendar-apply-filters={`${group.date}:${s.calendarResourceField ? resource.key : '__all__'}`}
											data-calendar-filters-applied={s.appliedCalendarFilterKey ===
											calendarBucketFilterKey(group.date, resource)
												? 'true'
												: undefined}
											><Filter class="h-3.5 w-3.5" aria-hidden="true" /></button
										>

										<span
											class="rounded bg-raised px-2 py-0.5 text-xs text-fg-muted"
											>{$t('forms.groupRecordCount', {
												values: { count: group.records.length }
											})}</span
										>
									</div>
								</div>

								<div class="space-y-2 p-2">
									{#each group.records as occurrence (occurrence.key)}
										{@const record = occurrence.record}

										<article
											class={`rounded-card border border-line bg-raised p-3 transition-opacity ${s.draggingCalendarRecordId === record.id ? 'opacity-60' : ''}`}
											draggable={Boolean(s.calendarDateField) &&
												!occurrence.recurring}
											data-calendar-card={record.id}
											data-calendar-occurrence={occurrence.spanStart
												? `${record.id}:${occurrence.startDate}`
												: occurrence.key}
											data-calendar-recurring={occurrence.recurring}
											data-calendar-start={occurrence.startDate}
											data-calendar-end={occurrence.endDate}
											data-calendar-span-start={occurrence.spanStart}
											data-calendar-span-end={occurrence.spanEnd}
											data-calendar-span-days={occurrence.spanDays}
											data-calendar-schedule-copied={s.copiedCalendarScheduleKey ===
											calendarScheduleKey(occurrence)
												? 'true'
												: undefined}
											data-calendar-shifted={s.shiftedCalendarScheduleKey ===
											calendarScheduleKey(occurrence)
												? 'true'
												: undefined}
											role="listitem"
											ondragstart={(event) =>
												handleCalendarDragStart(event, record)}
											ondragend={handleCalendarDragEnd}
										>
											<div class="flex items-start justify-between gap-2">
												<button
													type="button"
													class="min-w-0 text-left"
													onclick={() => selectRecord(record)}
													aria-label={`${$t('forms.openRecord')}: ${record.title}`}
												>
													<span
														class="block truncate text-sm font-medium text-fg"
														>{record.title}</span
													>

													<span
														class="mt-0.5 block truncate text-xs text-fg-muted"
													>
														{s.visibleFields[0]
															? recordDisplay(record, s.visibleFields[0])
															: record.id.slice(0, 8)}
													</span>

													{#if s.visibleFields[0] && recordFieldHiddenByCondition(s.visibleFields[0], record)}
														<span
															class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-calendar-summary-conditional-hidden={`${record.id}:${s.visibleFields[0].key}`}
															>{$t('forms.fieldConditionHiddenBadge')}</span
														>
													{/if}

													{#if s.visibleFields[0] && recordFieldReadOnlyByCondition(s.visibleFields[0], record)}
														<span
															class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-calendar-summary-conditional-readonly={`${record.id}:${s.visibleFields[0].key}`}
															>{$t('forms.fieldConditionReadOnlyBadge')}</span
														>
													{/if}
												</button>

												<div class="flex shrink-0 items-center gap-1">
													<button
														type="button"
														class="inline-flex items-center rounded-control p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
														onclick={() =>
															shiftCalendarOccurrence(occurrence, -1)}
														disabled={s.updatingCalendarRecordId ===
															record.id ||
															!canShiftCalendarOccurrence(occurrence)}
														aria-label={$t('forms.calendarShiftPreviousDay')}
														title={$t('forms.calendarShiftPreviousDay')}
														data-calendar-shift-previous={calendarScheduleKey(
															occurrence
														)}
														><ChevronLeft
															class="h-3.5 w-3.5"
															aria-hidden="true"
														/></button
													>

													<button
														type="button"
														class="inline-flex items-center rounded-control p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
														onclick={() =>
															shiftCalendarOccurrence(occurrence, 1)}
														disabled={s.updatingCalendarRecordId ===
															record.id ||
															!canShiftCalendarOccurrence(occurrence)}
														aria-label={$t('forms.calendarShiftNextDay')}
														title={$t('forms.calendarShiftNextDay')}
														data-calendar-shift-next={calendarScheduleKey(
															occurrence
														)}
														><ChevronRight
															class="h-3.5 w-3.5"
															aria-hidden="true"
														/></button
													>

													<button
														type="button"
														class="inline-flex items-center rounded-control p-1 text-fg-muted hover:bg-surface hover:text-fg"
														onclick={() => copyCalendarSchedule(occurrence)}
														aria-label={$t('forms.copyCalendarSchedule')}
														title={$t('forms.copyCalendarSchedule')}
														data-calendar-copy-schedule={calendarScheduleKey(
															occurrence
														)}
														><Copy
															class="h-3.5 w-3.5"
															aria-hidden="true"
														/></button
													>

													<button
														type="button"
														class="inline-flex items-center rounded-control p-1 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
														disabled={s.deletingRecordId === record.id}
														onclick={() => handleDeleteRecord(record)}
														aria-label={$t('forms.deleteRecord')}
														><Trash2
															class="h-3.5 w-3.5"
															aria-hidden="true"
														/></button
													>
												</div>
											</div>

											{#if occurrence.recurring}
												<div class="mt-3 space-y-2">
													<span
														class="inline-flex rounded bg-surface px-2 py-0.5 text-xs font-medium text-fg-muted"
														>{$t('forms.calendarRecurringOccurrence')}</span
													>

													{#if s.calendarDateField && ['date', 'datetime'].includes(s.calendarDateField.type ?? 'text')}{@const calendarField =
															s.calendarDateField}
														<label class="block space-y-1">
															<span
																class="text-[11px] font-medium uppercase text-fg-muted"
																>{$t('forms.calendarSeriesDate')}</span
															>

															<input
																type="date"
																value={calendarDateInputValue(
																	record,
																	s.calendarDateField
																)}
																disabled={s.updatingCalendarRecordId ===
																	record.id}
																aria-label={`${$t('forms.calendarSeriesDate')} ${record.title}`}
																data-calendar-series-date={`${record.id}:${occurrence.date}`}
																onchange={(event) =>
																	updateCalendarSeriesDate(
																		record,
																		calendarField,
																		inputValue(event)
																	)}
																class="w-full rounded-control border border-input bg-selected px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
															/>
														</label>
													{/if}
												</div>
											{/if}

											{#if occurrence.spanDays > 1}
												<div
													class="mt-3 inline-flex max-w-full items-center gap-1 rounded bg-success-soft px-2 py-0.5 text-xs font-medium text-success"
												>
													<span class="truncate">
														{$t('forms.calendarDateSpan', {
															values: {
																start: occurrence.startDate,
																end: occurrence.endDate
															}
														})}
													</span>
												</div>
											{/if}

											{#if s.calendarDateField && ['date', 'datetime'].includes(s.calendarDateField.type ?? 'text') && !occurrence.recurring}{@const calendarField =
													s.calendarDateField}
												<input
													type="date"
													value={calendarDateInputValue(
														record,
														s.calendarDateField
													)}
													disabled={s.updatingCalendarRecordId === record.id}
													aria-label={`${$t('forms.calendarSetDate')} ${fieldLabel(s.calendarDateField)}`}
													onchange={(event) =>
														updateCalendarRecordDate(
															record,
															calendarField,
															inputValue(event)
														)}
													class="mt-3 w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
												/>
											{/if}

											{#if s.calendarDateField && s.calendarEndField && ['date', 'datetime'].includes(s.calendarEndField.type ?? 'text') && occurrence.startDate === calendarRecordIsoDate(record, s.calendarDateField)}{@const calendarEnd =
													s.calendarEndField}
												<label class="mt-3 block space-y-1">
													<span
														class="text-[11px] font-medium uppercase text-fg-muted"
														>{$t('forms.calendarEndDate')}</span
													>

													<input
														type="date"
														value={calendarDateInputValue(
															record,
															s.calendarEndField
														)}
														disabled={s.updatingCalendarRecordId === record.id}
														aria-label={`${$t('forms.calendarEndDate')} ${record.title}`}
														data-calendar-end-date={`${record.id}:${occurrence.startDate}`}
														onchange={(event) =>
															updateCalendarRecordEndDate(
																record,
																calendarEnd,
																inputValue(event)
															)}
														class="w-full rounded-control border border-line bg-success-soft px-2 py-1.5 text-xs text-success disabled:bg-surface disabled:text-fg-muted"
													/>
												</label>
											{/if}
										</article>
									{/each}
								</div>
							</section>
						{/each}
					</div>
				{/if}
			</section>
		{/each}
	</div>
</div>
