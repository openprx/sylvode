<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ganttDateLabel } from '#lib/forms/calendar-dates.js';
	import {
		fieldHasConditionalHidden,
		fieldHasConditionalReadOnly,
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		extendGanttTaskEnd,
		handleGanttDragStart,
		handleGanttDragEnd,
		handleGanttDragOver,
		handleGanttDrop
	} = fp.viewGantt;
</script>

<div class="overflow-x-auto p-4" data-view-renderer="gantt">
	{#if !s.ganttTimeline.startField}
		<div
			class="rounded-card border border-dashed border-input bg-nav px-4 py-6 text-sm text-fg-muted"
		>
			{$t('forms.ganttConfigureFields')}
		</div>
	{:else}
		<div class="mb-3 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
			<span data-gantt-start-field={s.ganttTimeline.startField.key}
				>{$t('forms.ganttStartField')}: {fieldLabel(
					s.ganttTimeline.startField
				)}</span
			>

			{#if fieldHasConditionalHidden(s.ganttTimeline.startField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-start-field-conditional-hidden={s.ganttTimeline.startField
						.key}>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if fieldHasConditionalReadOnly(s.ganttTimeline.startField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-start-field-conditional-readonly={s.ganttTimeline
						.startField.key}>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>

			<span data-gantt-end-field={s.ganttTimeline.endField?.key ?? ''}>
				{$t('forms.ganttEndField')}: {s.ganttTimeline.endField
					? fieldLabel(s.ganttTimeline.endField)
					: $t('forms.ganttNoEndField')}
			</span>

			{#if s.ganttTimeline.endField && fieldHasConditionalHidden(s.ganttTimeline.endField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-end-field-conditional-hidden={s.ganttTimeline.endField.key}
					>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if s.ganttTimeline.endField && fieldHasConditionalReadOnly(s.ganttTimeline.endField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-end-field-conditional-readonly={s.ganttTimeline.endField
						.key}>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>

			<span data-gantt-group-field={s.ganttTimeline.groupField?.key ?? ''}>
				{$t('forms.ganttGroupField')}: {s.ganttTimeline.groupField
					? fieldLabel(s.ganttTimeline.groupField)
					: $t('forms.ganttUngrouped')}
			</span>

			{#if s.ganttTimeline.groupField && fieldHasConditionalHidden(s.ganttTimeline.groupField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-group-field-conditional-hidden={s.ganttTimeline.groupField
						.key}>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if s.ganttTimeline.groupField && fieldHasConditionalReadOnly(s.ganttTimeline.groupField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-group-field-conditional-readonly={s.ganttTimeline
						.groupField.key}>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}

			<span>·</span>

			<span
				data-gantt-dependency-field={s.ganttTimeline.dependencyField?.key ?? ''}
			>
				{$t('forms.ganttDependencyField')}: {s.ganttTimeline.dependencyField
					? fieldLabel(s.ganttTimeline.dependencyField)
					: $t('forms.viewNoRule')}
			</span>

			{#if s.ganttTimeline.dependencyField && fieldHasConditionalHidden(s.ganttTimeline.dependencyField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-dependency-field-conditional-hidden={s.ganttTimeline
						.dependencyField.key}>{$t('forms.fieldConditionHiddenBadge')}</span
				>
			{/if}

			{#if s.ganttTimeline.dependencyField && fieldHasConditionalReadOnly(s.ganttTimeline.dependencyField)}
				<span
					class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
					data-gantt-dependency-field-conditional-readonly={s.ganttTimeline
						.dependencyField.key}
					>{$t('forms.fieldConditionReadOnlyBadge')}</span
				>
			{/if}
		</div>

		{#if s.ganttTimeline.dates.length === 0}
			<div
				class="rounded-card border border-dashed border-input bg-nav px-4 py-6 text-sm text-fg-muted"
			>
				{$t('forms.noRecords')}
			</div>
		{:else}
			<div
				class="min-w-[980px] rounded-control border border-line bg-raised text-sm"
				data-gantt-scale={s.ganttTimeline.scale}
				data-gantt-start={s.ganttTimeline.dates[0]}
				data-gantt-end={s.ganttTimeline.dates[s.ganttTimeline.dates.length - 1]}
			>
				<div
					class="grid grid-cols-[180px_minmax(0,1fr)] border-b border-line bg-nav"
				>
					<div class="px-4 py-3 text-xs font-semibold uppercase text-fg-muted">
						{$t('forms.records')}
					</div>

					<div
						class="grid divide-x divide-line"
						style={`grid-template-columns: repeat(${s.ganttTimeline.dates.length}, minmax(72px, 1fr));`}
					>
						{#each s.ganttTimeline.dates as date}
							<div
								class="px-2 py-3 text-center text-xs font-medium text-fg-muted"
								data-gantt-date={date}
								role="button"
								tabindex="0"
								aria-label={`${$t('forms.ganttStartField')} ${date}`}
								ondragover={handleGanttDragOver}
								ondrop={(event) => handleGanttDrop(event, date)}
							>
								{ganttDateLabel(date, s.ganttTimeline.scale)}
							</div>
						{/each}
					</div>
				</div>

				{#each s.ganttTimeline.lanes as lane}
					<div
						class="border-b border-line last:border-b-0"
						data-gantt-lane={lane.label}
					>
						<div
							class="bg-nav px-4 py-2 text-xs font-semibold uppercase text-fg-muted"
						>
							{lane.label}
						</div>

						{#if lane.tasks.length === 0}
							<div class="grid grid-cols-[180px_minmax(0,1fr)]">
								<div class="px-4 py-3 text-xs text-fg-muted">
									{$t('forms.noRecords')}
								</div>
								<div class="min-h-10"></div>
							</div>
						{/if}

						{#each lane.tasks as task}
							<div
								class="grid grid-cols-[180px_minmax(0,1fr)] items-center border-t border-line"
							>
								<div class="min-w-0 px-4 py-3">
									<div class="truncate font-medium text-fg">
										{task.record.title}
									</div>

									<div class="mt-0.5 text-xs text-fg-muted">
										{task.start}{task.end !== task.start
											? ` - ${task.end}`
											: ''}
									</div>

									<div class="mt-1 flex flex-wrap items-center gap-1.5">
										{#if recordFieldHiddenByCondition(s.ganttTimeline.startField, task.record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-gantt-task-start-conditional-hidden={`${task.record.id}:${s.ganttTimeline.startField.key}`}
												>{$t('forms.fieldConditionHiddenBadge')}</span
											>
										{/if}

										{#if recordFieldReadOnlyByCondition(s.ganttTimeline.startField, task.record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-gantt-task-start-conditional-readonly={`${task.record.id}:${s.ganttTimeline.startField.key}`}
												>{$t('forms.fieldConditionReadOnlyBadge')}</span
											>
										{/if}

										{#if s.ganttTimeline.endField && recordFieldHiddenByCondition(s.ganttTimeline.endField, task.record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-gantt-task-end-conditional-hidden={`${task.record.id}:${s.ganttTimeline.endField.key}`}
												>{$t('forms.fieldConditionHiddenBadge')}</span
											>
										{/if}

										{#if s.ganttTimeline.endField && recordFieldReadOnlyByCondition(s.ganttTimeline.endField, task.record)}
											<span
												class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
												data-gantt-task-end-conditional-readonly={`${task.record.id}:${s.ganttTimeline.endField.key}`}
												>{$t('forms.fieldConditionReadOnlyBadge')}</span
											>
										{/if}
									</div>

									{#if s.ganttTimeline.dependencyField && task.dependencyValue && task.dependencyValue !== '-'}
										<div
											class="mt-1 truncate text-xs text-fg-muted"
											data-gantt-dependency={task.record.id}
										>
											{$t('forms.ganttDependency')}: {task.dependencyValue}
										</div>
									{/if}

									{#if s.ganttTimeline.dependencyField && recordFieldHiddenByCondition(s.ganttTimeline.dependencyField, task.record)}
										<span
											class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-gantt-task-dependency-conditional-hidden={`${task.record.id}:${s.ganttTimeline.dependencyField.key}`}
											>{$t('forms.fieldConditionHiddenBadge')}</span
										>
									{/if}

									{#if s.ganttTimeline.dependencyField && recordFieldReadOnlyByCondition(s.ganttTimeline.dependencyField, task.record)}
										<span
											class="mt-1 inline-flex rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
											data-gantt-task-dependency-conditional-readonly={`${task.record.id}:${s.ganttTimeline.dependencyField.key}`}
											>{$t('forms.fieldConditionReadOnlyBadge')}</span
										>
									{/if}
								</div>

								<div
									class="grid min-h-14 items-center gap-y-1 px-2"
									style={`grid-template-columns: repeat(${s.ganttTimeline.dates.length}, minmax(72px, 1fr));`}
								>
									<div
										class="flex items-center justify-between gap-2 rounded-control bg-success px-3 py-2 text-xs font-semibold text-on-success transition-opacity"
										style={`grid-column: ${task.startOffset + 1} / span ${task.span};`}
										draggable="true"
										role="button"
										tabindex="0"
										aria-label={task.record.title}
										ondragstart={(event) =>
											handleGanttDragStart(event, task.record)}
										ondragend={handleGanttDragEnd}
										data-gantt-bar={task.record.id}
										data-gantt-bar-start={task.start}
										data-gantt-bar-end={task.end}
										data-gantt-bar-dependency={task.dependencyValue}
										data-gantt-updating={s.updatingGanttRecordId ===
										task.record.id
											? 'true'
											: 'false'}
									>
										<span class="min-w-0 truncate">{task.record.title}</span>

										{#if s.ganttTimeline.dependencyField && task.dependencyValue && task.dependencyValue !== '-'}
											<span
												class="hidden shrink-0 rounded bg-on-success/15 px-1.5 py-0.5 text-[11px] font-medium text-on-success md:inline"
												data-gantt-bar-dependency-label={task.record.id}
												>{task.dependencyValue}</span
											>
										{/if}

										{#if s.ganttTimeline.endField}
											<button
												type="button"
												class="shrink-0 rounded bg-on-success/20 px-1.5 py-0.5 text-[11px] leading-none text-on-success hover:bg-on-success/30"
												onclick={(event) => {
													event.stopPropagation();
													extendGanttTaskEnd(task);
												}}
												disabled={s.updatingGanttRecordId === task.record.id}
												aria-label={$t('forms.ganttResizeEnd')}
												data-gantt-resize-end={task.record.id}>+1</button
											>
										{/if}
									</div>
								</div>
							</div>
						{/each}
					</div>
				{/each}
			</div>
		{/if}
	{/if}
</div>
