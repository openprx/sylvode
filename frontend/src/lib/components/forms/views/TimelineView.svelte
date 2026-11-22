<svelte:options runes />

<script lang="ts">
	import {
		ArrowUp,
		Copy,
		FileDown,
		FileText,
		Filter,
		Link,
		MoreVertical,
		Shield,
		TableProperties,
		Trash2
	} from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import {
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { formatDate, selectValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import { kanbanMoveOptions } from '#lib/forms/view-config.js';
	import {
		timelineEventAggregateKeys,
		timelineEventDetailsJson,
		timelineEventJson,
		timelineEventSubtitle,
		timelineEventTitle
	} from '#lib/forms/views/timeline-events.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		applyTimelineEventDayWindow,
		applyTimelineEventQuickFilter,
		copyTimelineAggregateIds,
		copyTimelineEventIds,
		copyTimelineEventValue,
		exportTimelineEventDetails,
		exportTimelineEventsCsv,
		exportTimelineEventsJson,
		focusTimelineEventRecord,
		focusTimelineEventRecords,
		selectedTimelineEventActor,
		selectedTimelineEventAggregate,
		selectedTimelineEventFrom,
		selectedTimelineEventLayout,
		selectedTimelineEventQuery,
		selectedTimelineEventTo,
		selectedTimelineEventType,
		selectedTimelineSource,
		timelineEventLinkedRecordIds,
		timelineEventPinned,
		timelineEventRecord,
		timelineSubtitle,
		timelineTitle,
		toggleTimelineEventPinned,
		updateTimelineRecordSelectField
	} = fp.viewTimeline;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const { handleDeleteRecord } = fp.records;
	const { selectRecord } = fp.recordPanel;
</script>

<div
	class="divide-y divide-line"
	data-view-renderer="timeline"
	data-timeline-source={selectedTimelineSource()}
	data-timeline-event-layout={selectedTimelineEventLayout()}
	data-timeline-event-type-filter={selectedTimelineEventType()}
	data-timeline-event-actor-filter={selectedTimelineEventActor()}
	data-timeline-event-aggregate-filter={selectedTimelineEventAggregate()}
	data-timeline-event-query={selectedTimelineEventQuery()}
	data-timeline-event-from={selectedTimelineEventFrom()}
	data-timeline-event-to={selectedTimelineEventTo()}
	data-timeline-event-pinned-ids={s.pinnedTimelineEventIds.join('|')}
	data-timeline-events-exported={s.exportedTimelineEventsCsvCount ?? undefined}
	data-timeline-events-json-exported={s.exportedTimelineEventsJsonCount ?? undefined}
	data-timeline-event-ids-copied={s.copiedTimelineEventIdsCount ?? undefined}
	data-timeline-aggregate-ids-copied={s.copiedTimelineAggregateIdsCount ?? undefined}
>
	{#if selectedTimelineSource() === 'events'}
		{#if s.eventsLoading}
			<div class="px-4 py-6 text-sm text-fg-muted">
				{$t('common.loading')}
			</div>
		{:else if s.timelineEvents.length === 0}
			<div class="px-4 py-6 text-sm text-fg-muted">
				{$t('forms.noRecords')}
			</div>
		{:else}
			<div
				class="flex items-center justify-between gap-3 border-b border-line px-4 py-2"
				data-timeline-event-export-toolbar
			>
				<div class="min-w-0 text-xs text-fg-muted">
					{$t('forms.groupRecordCount', {
						values: { count: s.timelineEventRows.length }
					})}
				</div>

				<div class="flex shrink-0 flex-wrap items-center justify-end gap-2">
					<button
						type="button"
						class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
						disabled={s.timelineEventRows.length === 0}
						onclick={() => copyTimelineEventIds(s.timelineEventRows)}
						data-timeline-copy-event-ids
					>
						<Copy class="h-3.5 w-3.5" aria-hidden="true" />
						{$t('forms.copyTimelineEventIds')}
					</button>

					<button
						type="button"
						class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
						disabled={timelineEventAggregateKeys(s.timelineEventRows).length === 0}
						onclick={() => copyTimelineAggregateIds(s.timelineEventRows)}
						data-timeline-copy-aggregate-ids
					>
						<Link class="h-3.5 w-3.5" aria-hidden="true" />
						{$t('forms.copyTimelineAggregateIds')}
					</button>

					<button
						type="button"
						class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
						disabled={timelineEventLinkedRecordIds(s.timelineEventRows).length === 0}
						onclick={() => focusTimelineEventRecords(s.timelineEventRows)}
						data-timeline-focus-event-records
					>
						<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
						{$t('forms.focusTimelineEventRecords')}
					</button>

					<button
						type="button"
						class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
						disabled={s.timelineEventRows.length === 0}
						onclick={() => exportTimelineEventsJson(s.timelineEventRows)}
						data-timeline-export-events-json
					>
						<FileText class="h-3.5 w-3.5" aria-hidden="true" />
						{$t('forms.exportTimelineEventsJson')}
					</button>

					<button
						type="button"
						class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
						disabled={s.timelineEventRows.length === 0}
						onclick={() => exportTimelineEventsCsv(s.timelineEventRows)}
						data-timeline-export-events-csv
					>
						<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
						{$t('forms.exportTimelineEventsCsv')}
					</button>
				</div>
			</div>

			{#each s.timelineEventRows as event (event.id)}
				{@const linkedRecord = timelineEventRecord(event)}
				{@const pinned = timelineEventPinned(event)}

				<article
					class="grid grid-cols-[8rem_minmax(0,1fr)] gap-4 px-4 py-3"
					data-timeline-event={event.id}
					data-timeline-event-type={event.event_type}
					data-timeline-event-pinned={pinned ? 'true' : 'false'}
				>
					<div class="text-xs text-fg-muted">
						{formatDate(event.created_at)}
					</div>

					<div class="min-w-0 border-l border-line pl-4">
						<div class="flex items-start justify-between gap-3">
							<div class="min-w-0">
								<span
									class="block truncate font-medium text-fg"
									data-timeline-event-title={event.id}>{timelineEventTitle(event)}</span
								>

								<span
									class="mt-1 block truncate text-xs text-fg-muted"
									data-timeline-event-subtitle={event.id}>{timelineEventSubtitle(event)}</span
								>
							</div>

							<div class="flex shrink-0 items-center gap-2">
								<span class="rounded bg-surface px-2 py-0.5 text-xs font-medium text-fg-muted"
									>{event.event_type}</span
								>

								{#if pinned}
									<span class="rounded bg-warning-soft px-2 py-0.5 text-xs font-medium text-warning"
										>{$t('forms.timelineEventPinnedBadge')}</span
									>
								{/if}

								<details
									class="relative"
									data-timeline-event-menu={event.id}
									data-timeline-event-copied={s.copiedTimelineEventAction?.startsWith(
										`${event.id}:`
									)
										? s.copiedTimelineEventAction
										: undefined}
									data-timeline-event-exported={s.exportedTimelineEventAction?.startsWith(
										`${event.id}:`
									)
										? s.exportedTimelineEventAction
										: undefined}
								>
									<summary
										class="inline-flex cursor-pointer list-none items-center rounded-control p-1 text-fg-muted hover:bg-surface hover:text-fg"
										aria-label={$t('forms.timelineEventActions')}
										><MoreVertical class="h-4 w-4" aria-hidden="true" /></summary
									>

									<div
										class="absolute right-0 z-20 mt-1 w-48 overflow-hidden rounded-float border border-line bg-overlay py-1 text-sm shadow-overlay"
									>
										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
											disabled={!linkedRecord}
											data-timeline-event-action={`${event.id}:open-record`}
											onclick={() => {
												if (linkedRecord) void selectRecord(linkedRecord);
											}}
										>
											<Link class="h-4 w-4" aria-hidden="true" />
											{$t('forms.openLinkedRecord')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
											disabled={!linkedRecord}
											data-timeline-event-action={`${event.id}:focus-record`}
											onclick={() => focusTimelineEventRecord(event)}
										>
											<TableProperties class="h-4 w-4" aria-hidden="true" />
											{$t('forms.focusLinkedRecord')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:${pinned ? 'unpin-event' : 'pin-event'}`}
											onclick={() => toggleTimelineEventPinned(event)}
										>
											<ArrowUp class="h-4 w-4" aria-hidden="true" />

											{pinned ? $t('forms.unpinTimelineEvent') : $t('forms.pinTimelineEvent')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:filter-type`}
											onclick={() => applyTimelineEventQuickFilter(event, 'type')}
										>
											<FileText class="h-4 w-4" aria-hidden="true" />
											{$t('forms.timelineQuickFilterType')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
											disabled={!event.actor_id}
											data-timeline-event-action={`${event.id}:filter-actor`}
											onclick={() => applyTimelineEventQuickFilter(event, 'actor')}
										>
											<Shield class="h-4 w-4" aria-hidden="true" />
											{$t('forms.timelineQuickFilterActor')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:filter-aggregate`}
											onclick={() => applyTimelineEventQuickFilter(event, 'aggregate')}
										>
											<Link class="h-4 w-4" aria-hidden="true" />
											{$t('forms.timelineQuickFilterAggregate')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:filter-day`}
											onclick={() => applyTimelineEventDayWindow(event)}
										>
											<Filter class="h-4 w-4" aria-hidden="true" />
											{$t('forms.timelineQuickFilterDay')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:copy-event-id`}
											onclick={() => copyTimelineEventValue(event, event.id, 'event-id')}
										>
											<Copy class="h-4 w-4" aria-hidden="true" />
											{$t('forms.copyEventId')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:copy-aggregate`}
											onclick={() =>
												copyTimelineEventValue(
													event,
													`${event.aggregate_type}:${event.aggregate_id}`,
													'aggregate'
												)}
										>
											<Copy class="h-4 w-4" aria-hidden="true" />
											{$t('forms.copyAggregateId')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:copy-details`}
											onclick={() =>
												copyTimelineEventValue(event, timelineEventDetailsJson(event), 'details')}
										>
											<Copy class="h-4 w-4" aria-hidden="true" />
											{$t('forms.copyEventDetails')}
										</button>

										<button
											type="button"
											class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
											data-timeline-event-action={`${event.id}:export-details`}
											onclick={() => exportTimelineEventDetails(event)}
										>
											<FileDown class="h-4 w-4" aria-hidden="true" />
											{$t('forms.exportEventDetails')}
										</button>
									</div>
								</details>
							</div>
						</div>

						<details
							class="mt-3 rounded-card border border-line bg-nav"
							data-timeline-event-detail={event.id}
							open={selectedTimelineEventLayout() === 'detailed'}
						>
							<summary
								class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 text-xs font-medium text-fg hover:bg-surface"
							>
								<span>{$t('forms.timelineEventDetails')}</span>
								<span class="text-fg-muted">{event.id}</span>
							</summary>

							<div class="space-y-3 border-t border-line p-3 text-xs">
								<div class="grid gap-2 sm:grid-cols-2">
									<div>
										<div class="font-medium text-fg-muted">
											{$t('forms.timelineEventAggregate')}
										</div>

										<div class="mt-0.5 break-all text-fg" data-timeline-event-aggregate={event.id}>
											{event.aggregate_type}:{event.aggregate_id}
										</div>
									</div>

									<div>
										<div class="font-medium text-fg-muted">
											{$t('forms.timelineEventActor')}
										</div>

										<div class="mt-0.5 break-all text-fg" data-timeline-event-actor={event.id}>
											{event.actor_id || '-'}
										</div>
									</div>
								</div>

								<div class="grid gap-3 lg:grid-cols-3">
									<div>
										<div class="mb-1 font-medium text-fg-muted">
											{$t('forms.timelineEventPayload')}
										</div>

										<pre
											class="max-h-44 overflow-auto rounded bg-raised p-2 text-[11px] leading-relaxed text-fg"
											data-timeline-event-payload={event.id}>{timelineEventJson(
												event.payload
											)}</pre>
									</div>

									<div>
										<div class="mb-1 font-medium text-fg-muted">
											{$t('forms.timelineEventMetadata')}
										</div>

										<pre
											class="max-h-44 overflow-auto rounded bg-raised p-2 text-[11px] leading-relaxed text-fg"
											data-timeline-event-metadata={event.id}>{timelineEventJson(
												event.metadata
											)}</pre>
									</div>

									<div>
										<div class="mb-1 font-medium text-fg-muted">
											{$t('forms.timelineEventSource')}
										</div>

										<pre
											class="max-h-44 overflow-auto rounded bg-raised p-2 text-[11px] leading-relaxed text-fg"
											data-timeline-event-source-detail={event.id}>{timelineEventJson(
												event.source
											)}</pre>
									</div>
								</div>
							</div>
						</details>
					</div>
				</article>
			{/each}
		{/if}
	{:else}
		{#each s.timelineRecords as record (record.id)}
			<article class="grid grid-cols-[8rem_minmax(0,1fr)] gap-4 px-4 py-3">
				<div class="text-xs text-fg-muted">
					{formatDate(record.updated_at)}
				</div>

				<div class="min-w-0 border-l border-line pl-4">
					<div class="flex items-start justify-between gap-3">
						<button
							type="button"
							class="min-w-0 text-left"
							onclick={() => selectRecord(record)}
							aria-label={`${$t('forms.openRecord')}: ${record.title}`}
						>
							<span class="block truncate font-medium text-fg" data-timeline-title={record.id}
								>{timelineTitle(record)}</span
							>

							{#if s.timelineTitlePermissionField}
								<span class="mt-1 flex flex-wrap items-center gap-1.5">
									{#if memberFieldHidden(s.timelineTitlePermissionField)}
										<span
											class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
											data-timeline-title-permission-hidden={`${record.id}:${s.timelineTitlePermissionField.key}`}
											>{$t('forms.fieldHiddenBadge')}</span
										>
									{/if}

									{#if memberFieldLocked(s.timelineTitlePermissionField)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-title-permission-locked={`${record.id}:${s.timelineTitlePermissionField.key}`}
											>{$t('forms.fieldLockedBadge')}</span
										>
									{/if}

									{#if recordFieldHiddenByCondition(s.timelineTitlePermissionField, record)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-title-conditional-hidden={`${record.id}:${s.timelineTitlePermissionField.key}`}
											>{$t('forms.fieldConditionHiddenBadge')}</span
										>
									{/if}

									{#if recordFieldReadOnlyByCondition(s.timelineTitlePermissionField, record)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-title-conditional-readonly={`${record.id}:${s.timelineTitlePermissionField.key}`}
											>{$t('forms.fieldConditionReadOnlyBadge')}</span
										>
									{/if}
								</span>
							{/if}

							<span
								class="mt-1 block truncate text-xs text-fg-muted"
								data-timeline-subtitle={record.id}>{timelineSubtitle(record)}</span
							>

							{#if s.timelineSubtitlePermissionField}
								<span class="mt-1 flex flex-wrap items-center gap-1.5">
									{#if memberFieldHidden(s.timelineSubtitlePermissionField)}
										<span
											class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
											data-timeline-subtitle-permission-hidden={`${record.id}:${s.timelineSubtitlePermissionField.key}`}
											>{$t('forms.fieldHiddenBadge')}</span
										>
									{/if}

									{#if memberFieldLocked(s.timelineSubtitlePermissionField)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-subtitle-permission-locked={`${record.id}:${s.timelineSubtitlePermissionField.key}`}
											>{$t('forms.fieldLockedBadge')}</span
										>
									{/if}

									{#if recordFieldHiddenByCondition(s.timelineSubtitlePermissionField, record)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-subtitle-conditional-hidden={`${record.id}:${s.timelineSubtitlePermissionField.key}`}
											>{$t('forms.fieldConditionHiddenBadge')}</span
										>
									{/if}

									{#if recordFieldReadOnlyByCondition(s.timelineSubtitlePermissionField, record)}
										<span
											class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-timeline-subtitle-conditional-readonly={`${record.id}:${s.timelineSubtitlePermissionField.key}`}
											>{$t('forms.fieldConditionReadOnlyBadge')}</span
										>
									{/if}
								</span>
							{/if}
						</button>

						<button
							type="button"
							class="inline-flex shrink-0 items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
							disabled={s.deletingRecordId === record.id}
							onclick={() => handleDeleteRecord(record)}
							aria-label={$t('forms.deleteRecord')}
							><Trash2 class="h-4 w-4" aria-hidden="true" /></button
						>
					</div>

					{#if s.timelineEditableSelectFields.length > 0}
						<div class="mt-3 grid gap-2 sm:grid-cols-2">
							{#each s.timelineEditableSelectFields as field (field.key)}
								<label class="block space-y-1">
									<span
										class="flex flex-wrap items-center gap-1.5 text-xs font-medium text-fg-muted"
									>
										<span>{fieldLabel(field)}</span>

										{#if memberFieldHidden(field)}
											<span
												class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
												data-timeline-select-permission-hidden={`${record.id}:${field.key}`}
												>{$t('forms.fieldHiddenBadge')}</span
											>
										{/if}

										{#if memberFieldLocked(field)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-timeline-select-permission-locked={`${record.id}:${field.key}`}
												>{$t('forms.fieldLockedBadge')}</span
											>
										{/if}

										{#if recordFieldHiddenByCondition(field, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-timeline-select-conditional-hidden={`${record.id}:${field.key}`}
												>{$t('forms.fieldConditionHiddenBadge')}</span
											>
										{/if}

										{#if recordFieldReadOnlyByCondition(field, record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-timeline-select-conditional-readonly={`${record.id}:${field.key}`}
												>{$t('forms.fieldConditionReadOnlyBadge')}</span
											>
										{/if}
									</span>

									<select
										value={String(record.values[field.key] ?? '')}
										disabled={s.updatingTimelineRecordId === record.id}
										aria-label={`${$t('forms.timelineUpdateField')} ${fieldLabel(field)}`}
										data-timeline-select={`${record.id}:${field.key}`}
										onchange={(event) =>
											updateTimelineRecordSelectField(record, field, selectValue(event))}
										class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
									>
										{#each kanbanMoveOptions(field) as option, index (index)}
											<option value={option}>{option}</option>
										{/each}
									</select>
								</label>
							{/each}
						</div>
					{/if}
				</div>
			</article>
		{/each}
	{/if}
</div>
