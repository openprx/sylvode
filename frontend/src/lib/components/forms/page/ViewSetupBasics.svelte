<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { FormView } from '#lib/api/forms.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import type { ViewTimelineSource } from '#lib/forms/page-types.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import { viewCardCoverAspect, viewCardCoverFit, viewCardCoverPosition } from '#lib/forms/view-config.js';
	import { timelineEventLayout } from '#lib/forms/views/timeline-events.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
</script>

													<div class="grid grid-cols-2 gap-2">
														<label class="space-y-1">
															<span class="text-xs font-medium uppercase text-fg-muted"
																>{$t('forms.viewType')}</span
															>

															<select
																value={s.viewDraft.view_type}
																onchange={(event) =>
																	(s.viewDraft = {
																		...s.viewDraft,
																		view_type: selectValue(event) as FormView['view_type'],
																		pivotRowField:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotRowField
																				: '',
																		pivotColumnField:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotColumnField
																				: '',
																		pivotValueField:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotValueField
																				: '',
																		pivotAggregate:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotAggregate
																				: 'sum',
																		pivotTotals:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotTotals
																				: 'all',
																		pivotValueDisplay:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotValueDisplay
																				: 'value',
																		pivotSort:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotSort
																				: 'label',
																		pivotEmptyBuckets:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotEmptyBuckets
																				: false,
																		pivotHideZeroBuckets:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotHideZeroBuckets
																				: false,
																		pivotShowCounts:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotShowCounts
																				: false,
																		pivotHeatmap:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotHeatmap
																				: false,
																		pivotRowLimit:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotRowLimit
																				: 0,
																		pivotColumnLimit:
																			selectValue(event) === 'pivot'
																				? s.viewDraft.pivotColumnLimit
																				: 0,
																		ganttStartField:
																			selectValue(event) === 'gantt'
																				? s.viewDraft.ganttStartField
																				: '',
																		ganttEndField:
																			selectValue(event) === 'gantt'
																				? s.viewDraft.ganttEndField
																				: '',
																		ganttGroupBy:
																			selectValue(event) === 'gantt'
																				? s.viewDraft.ganttGroupBy
																				: '',
																		ganttDependencyField:
																			selectValue(event) === 'gantt'
																				? s.viewDraft.ganttDependencyField
																				: '',
																		ganttScale:
																			selectValue(event) === 'gantt'
																				? s.viewDraft.ganttScale
																				: 'day',
																		swimlaneBy:
																			selectValue(event) === 'kanban' ? s.viewDraft.swimlaneBy : '',
																		calendarRange:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarRange
																				: 'week',
																		calendarFocusDate:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarFocusDate
																				: '',
																		calendarRecurrence:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarRecurrence
																				: 'none',
																		calendarDayScope:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarDayScope
																				: 'all',
																		calendarLayout:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarLayout
																				: 'grid',
																		calendarHideEmptyDays:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarHideEmptyDays
																				: false,
																		calendarEndField:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarEndField
																				: '',
																		calendarResourceBy:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarResourceBy
																				: '',
																		calendarExceptionRecordId:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarExceptionRecordId
																				: '',
																		calendarExceptionDate:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarExceptionDate
																				: '',
																		calendarExceptionDates:
																			selectValue(event) === 'calendar'
																				? s.viewDraft.calendarExceptionDates
																				: {},
																		cardTitleField:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardTitleField
																				: '',
																		cardSubtitleField:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardSubtitleField
																				: '',
																		cardBadgeField:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardBadgeField
																				: '',
																		cardCoverField:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardCoverField
																				: '',
																		cardLayout:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardLayout
																				: 'standard',
																		cardCoverAspect:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardCoverAspect
																				: 'auto',
																		cardCoverFit:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardCoverFit
																				: 'cover',
																		cardCoverPosition:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardCoverPosition
																				: 'top',
																		cardFieldLimit:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardFieldLimit
																				: 6,
																		cardHideEmptyFields:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardHideEmptyFields
																				: false,
																		cardShowTimestamps:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardShowTimestamps
																				: false,
																		cardShowRecordId:
																			selectValue(event) === 'card'
																				? s.viewDraft.cardShowRecordId
																				: false,
																		timelineSource:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineSource
																				: 'records',
																		timelineEventLayout:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventLayout
																				: 'summary',
																		timelineEventType:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventType
																				: '',
																		timelineEventActor:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventActor
																				: '',
																		timelineEventAggregate:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventAggregate
																				: '',
																		timelineEventQuery:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventQuery
																				: '',
																		timelineEventFrom:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventFrom
																				: '',
																		timelineEventTo:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineEventTo
																				: '',
																		timelineTitleField:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineTitleField
																				: '',
																		timelineSubtitleField:
																			selectValue(event) === 'timeline'
																				? s.viewDraft.timelineSubtitleField
																				: '',
																		wipLimits:
																			selectValue(event) === 'kanban' ? s.viewDraft.wipLimits : {}
																	})}
																class="w-full rounded-card border border-input bg-raised px-3 py-2 text-sm text-fg"
															>
																<option value="grid">Grid</option>
																<option value="detail">Detail</option>
																<option value="kanban">Kanban</option>
																<option value="calendar">Calendar</option>
																<option value="card">Card</option>
																<option value="timeline">Timeline</option>
																<option value="pivot">Pivot</option>
																<option value="gantt">Gantt</option>
															</select>
														</label>

														{#if s.viewDraft.view_type === 'timeline'}
															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineSource')}</span
																>

																<select
																	value={s.viewDraft.timelineSource}
																	onchange={(event) => {
																		const timelineSource = selectValue(event) as ViewTimelineSource;

																		s.viewDraft = {
																			...s.viewDraft,
																			timelineSource,
																			timelineEventLayout:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventLayout
																					: 'summary',
																			timelineEventType:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventType
																					: '',
																			timelineEventActor:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventActor
																					: '',
																			timelineEventAggregate:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventAggregate
																					: '',
																			timelineEventQuery:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventQuery
																					: '',
																			timelineEventFrom:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventFrom
																					: '',
																			timelineEventTo:
																				timelineSource === 'events'
																					? s.viewDraft.timelineEventTo
																					: '',
																			timelineTitleField:
																				timelineSource === 'events'
																					? ''
																					: s.viewDraft.timelineTitleField,
																			timelineSubtitleField:
																				timelineSource === 'events'
																					? ''
																					: s.viewDraft.timelineSubtitleField
																		};
																	}}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="records"
																		>{$t('forms.timelineSourceRecords')}</option
																	>
																	<option value="events">{$t('forms.timelineSourceEvents')}</option>
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventLayout')}</span
																>

																<select
																	value={s.viewDraft.timelineEventLayout}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventLayout: timelineEventLayout(selectValue(event))
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="summary"
																		>{$t('forms.timelineEventLayoutSummary')}</option
																	>
																	<option value="detailed"
																		>{$t('forms.timelineEventLayoutDetailed')}</option
																	>
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventType')}</span
																>

																<select
																	value={s.viewDraft.timelineEventType}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventType: selectValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="">{$t('forms.timelineEventTypeAll')}</option>

																	{#each s.timelineEventTypes as eventType}
																		<option value={eventType}>{eventType}</option>
																	{/each}
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventActorFilter')}</span
																>

																<select
																	value={s.viewDraft.timelineEventActor}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventActor: selectValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="">{$t('forms.timelineEventActorAll')}</option>

																	{#each s.timelineEventActors as actorId}
																		<option value={actorId}>{actorId}</option>
																	{/each}
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventAggregateFilter')}</span
																>

																<select
																	value={s.viewDraft.timelineEventAggregate}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventAggregate: selectValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="">{$t('forms.timelineEventAggregateAll')}</option>

																	{#each s.timelineEventAggregates as aggregate}
																		<option value={aggregate}>{aggregate}</option>
																	{/each}
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventSearch')}</span
																>

																<input
																	value={s.viewDraft.timelineEventQuery}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	oninput={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventQuery: inputValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	placeholder={$t('forms.timelineEventSearchPlaceholder')}
																/>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventFrom')}</span
																>

																<input
																	type="datetime-local"
																	value={s.viewDraft.timelineEventFrom}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	oninput={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventFrom: inputValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																/>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineEventTo')}</span
																>

																<input
																	type="datetime-local"
																	value={s.viewDraft.timelineEventTo}
																	disabled={s.viewDraft.timelineSource !== 'events'}
																	oninput={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineEventTo: inputValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																/>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineTitleField')}</span
																>

																<select
																	value={s.viewDraft.timelineTitleField}
																	disabled={s.viewDraft.timelineSource === 'events'}
																	onchange={(event) => {
																		const timelineTitleField = selectValue(event);

																		s.viewDraft = {
																			...s.viewDraft,
																			timelineTitleField,
																			timelineSubtitleField:
																				s.viewDraft.timelineSubtitleField === timelineTitleField
																					? ''
																					: s.viewDraft.timelineSubtitleField
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
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.timelineSubtitleField')}</span
																>

																<select
																	value={s.viewDraft.timelineSubtitleField}
																	disabled={s.viewDraft.timelineSource === 'events'}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			timelineSubtitleField: selectValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="">{$t('forms.viewNoRule')}</option>

																	{#each s.fields.filter((field) => field.visibility?.list !== false && field.key !== s.viewDraft.timelineTitleField) as field}
																		<option value={field.key}>{fieldLabel(field)}</option>
																	{/each}
																</select>
															</label>
														{/if}

														{#if s.viewDraft.view_type === 'card'}
															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.cardCoverAspect')}</span
																>

																<select
																	value={s.viewDraft.cardCoverAspect}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			cardCoverAspect: viewCardCoverAspect(selectValue(event))
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="auto">{$t('forms.cardCoverAspectAuto')}</option>
																	<option value="wide">{$t('forms.cardCoverAspectWide')}</option>
																	<option value="square">{$t('forms.cardCoverAspectSquare')}</option
																	>
																	<option value="portrait"
																		>{$t('forms.cardCoverAspectPortrait')}</option
																	>
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.cardCoverFit')}</span
																>

																<select
																	value={s.viewDraft.cardCoverFit}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			cardCoverFit: viewCardCoverFit(selectValue(event))
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="cover">{$t('forms.cardCoverFitCover')}</option>
																	<option value="contain">{$t('forms.cardCoverFitContain')}</option>
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.cardCoverPosition')}</span
																>

																<select
																	value={s.viewDraft.cardCoverPosition}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			cardCoverPosition: viewCardCoverPosition(selectValue(event))
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="top">{$t('forms.cardCoverPositionTop')}</option>
																	<option value="left">{$t('forms.cardCoverPositionLeft')}</option>
																	<option value="hidden"
																		>{$t('forms.cardCoverPositionHidden')}</option
																	>
																</select>
															</label>
														{/if}
													</div>
