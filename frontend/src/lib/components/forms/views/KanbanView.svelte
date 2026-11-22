<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Trash2 } from '@lucide/svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import OptionValueBadges from '#lib/components/forms/OptionValueBadges.svelte';
	import {
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { selectValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import { kanbanMoveOptions } from '#lib/forms/view-config.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleDeleteRecord } = fp.records;
	const { selectRecord } = fp.recordPanel;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const {
		selectedKanbanRecords,
		kanbanBulkMoveAllowed,
		bulkMoveKanbanRecords,
		clearKanbanSelection,
		kanbanSwimlaneRows,
		kanbanColumnWipExceeded,
		kanbanWipLabel,
		kanbanColumnRecordCount,
		handleKanbanDragStart,
		handleKanbanDragEnd,
		handleKanbanDragOver,
		handleKanbanDrop,
		isKanbanRecordSelected,
		setKanbanRecordSelected,
		updateKanbanRecordField
	} = fp.viewKanban;
</script>

<div class="overflow-x-auto p-4" data-view-renderer="kanban">
	<div class="mb-3 text-xs text-fg-muted">
		{$t('forms.viewGroupBy', {
			values: {
				field: s.kanbanGroupField ? fieldLabel(s.kanbanGroupField) : $t('forms.recordTitle')
			}
		})}
	</div>

	{#if s.kanbanGroupField && kanbanMoveOptions(s.kanbanGroupField).length > 0}
		<div
			class="mb-3 flex min-w-[760px] flex-wrap items-end gap-2 rounded-control border border-line bg-raised p-2"
		>
			<div class="min-w-40 flex-1 text-xs text-fg-muted">
				<span class="font-medium text-fg"
					>{$t('forms.kanbanSelected', {
						values: { count: selectedKanbanRecords().length }
					})}</span
				>
			</div>

			<label class="min-w-56 space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.kanbanBulkTarget', {
						values: { field: fieldLabel(s.kanbanGroupField) }
					})}</span
				>

				<select
					value={s.bulkKanbanTarget}
					onchange={(event) => (s.bulkKanbanTarget = selectValue(event))}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="">{$t('forms.viewNoRule')}</option>

					{#each kanbanMoveOptions(s.kanbanGroupField) as option, index (index)}
						<option value={option}>{option}</option>
					{/each}
				</select>
			</label>

			<Button
				variant="primary"
				size="sm"
				onclick={bulkMoveKanbanRecords}
				loading={s.bulkMovingKanban}
				disabled={selectedKanbanRecords().length === 0 ||
					!s.bulkKanbanTarget ||
					!kanbanBulkMoveAllowed(selectedKanbanRecords(), s.bulkKanbanTarget)}
				>{$t('forms.kanbanBulkMove')}</Button
			>

			<Button
				size="sm"
				variant="secondary"
				onclick={clearKanbanSelection}
				disabled={selectedKanbanRecords().length === 0 || s.bulkMovingKanban}
				>{$t('forms.clearSelection')}</Button
			>
		</div>
	{/if}

	<div class="min-w-[760px] space-y-4">
		{#each kanbanSwimlaneRows() as swimlane, index (index)}
			<section
				class={s.kanbanSwimlaneField ? 'rounded-card border border-line bg-surface p-3' : ''}
				data-kanban-swimlane={s.kanbanSwimlaneField ? swimlane.label : undefined}
				data-record-count={swimlane.records.length}
			>
				{#if s.kanbanSwimlaneField}
					<div class="mb-3 flex items-center justify-between gap-3">
						<div class="min-w-0">
							<div class="text-xs font-medium uppercase text-fg-muted">
								{$t('forms.viewSwimlaneBy', {
									values: { field: fieldLabel(s.kanbanSwimlaneField) }
								})}
							</div>

							<h4 class="truncate text-sm font-semibold text-fg">
								{swimlane.label}
							</h4>
						</div>

						<span class="rounded bg-raised px-2 py-0.5 text-xs text-fg-muted"
							>{$t('forms.groupRecordCount', {
								values: { count: swimlane.records.length }
							})}</span
						>
					</div>
				{/if}

				<div class="grid gap-3 lg:grid-cols-3 xl:grid-cols-4">
					{#each swimlane.groups as group, index (index)}
						<section
							class={`rounded-card border bg-nav transition-colors ${
								kanbanColumnWipExceeded(group.label) ? 'border-line' : 'border-line'
							}`}
							data-record-count={group.records.length}
							data-kanban-column={group.label}
							data-wip-limited={kanbanColumnWipExceeded(group.label)}
							role="list"
							ondragover={(event) => handleKanbanDragOver(event, group.label)}
							ondrop={(event) => handleKanbanDrop(event, group.label)}
						>
							<div class="flex items-center justify-between gap-3 border-b border-line px-3 py-2">
								<h4 class="truncate text-sm font-semibold text-fg">
									{group.label}
								</h4>

								<span
									class={`rounded px-2 py-0.5 text-xs ${
										kanbanColumnWipExceeded(group.label)
											? 'bg-warning-soft text-warning'
											: 'bg-raised text-fg-muted'
									}`}>{kanbanWipLabel(group.label, kanbanColumnRecordCount(group.label))}</span
								>
							</div>

							<div class="space-y-2 p-2">
								{#each group.records as record (record.id)}
									<article
										class={`rounded-card border border-line bg-raised p-3 transition-opacity ${s.draggingKanbanRecordId === record.id ? 'opacity-60' : ''}`}
										draggable={Boolean(s.kanbanGroupField)}
										data-kanban-card={record.id}
										role="listitem"
										ondragstart={(event) => handleKanbanDragStart(event, record)}
										ondragend={handleKanbanDragEnd}
									>
										<div class="flex items-start justify-between gap-2">
											<div class="flex min-w-0 items-start gap-2">
												<input
													type="checkbox"
													class="mt-0.5 h-4 w-4 rounded border-input accent-primary"
													checked={isKanbanRecordSelected(record.id)}
													disabled={s.bulkMovingKanban}
													onchange={(event) =>
														setKanbanRecordSelected(
															record.id,
															event.currentTarget instanceof HTMLInputElement
																? event.currentTarget.checked
																: false
														)}
													aria-label={`${$t('forms.kanbanSelectCard')}: ${record.title}`}
												/>

												<button
													type="button"
													class="min-w-0 text-left text-sm font-medium text-fg"
													onclick={() => selectRecord(record)}
													aria-label={`${$t('forms.openRecord')}: ${record.title}`}
													><span class="block truncate">{record.title}</span></button
												>
											</div>

											<button
												type="button"
												class="inline-flex shrink-0 items-center rounded-control p-1 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
												disabled={s.deletingRecordId === record.id}
												onclick={() => handleDeleteRecord(record)}
												aria-label={$t('forms.deleteRecord')}
												><Trash2 class="h-3.5 w-3.5" aria-hidden="true" /></button
											>
										</div>

										<div class="mt-2 space-y-1">
											{#each s.cardFields.slice(0, 3) as field (field.key)}
												<p class="truncate text-xs text-fg-muted">
													<span class="inline-flex items-center gap-1 font-medium">
														<span>{fieldLabel(field)}:</span>

														{#if memberFieldHidden(field)}
															<span
																class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
																data-kanban-card-field-permission-hidden={`${record.id}:${field.key}`}
																>{$t('forms.fieldHiddenBadge')}</span
															>
														{/if}

														{#if memberFieldLocked(field)}
															<span
																class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																data-kanban-card-field-permission-locked={`${record.id}:${field.key}`}
																>{$t('forms.fieldLockedBadge')}</span
															>
														{/if}

														{#if recordFieldHiddenByCondition(field, record)}
															<span
																class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																data-kanban-card-field-conditional-hidden={`${record.id}:${field.key}`}
																>{$t('forms.fieldConditionHiddenBadge')}</span
															>
														{/if}

														{#if recordFieldReadOnlyByCondition(field, record)}
															<span
																class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																data-kanban-card-field-conditional-readonly={`${record.id}:${field.key}`}
																>{$t('forms.fieldConditionReadOnlyBadge')}</span
															>
														{/if}
													</span>

													<OptionValueBadges {field} value={record.values[field.key]} />
												</p>
											{/each}
										</div>

										{#if s.kanbanGroupField && kanbanMoveOptions(s.kanbanGroupField).length > 0}{@const kanbanField =
												s.kanbanGroupField}
											<select
												value={String(record.values[s.kanbanGroupField.key] ?? '')}
												disabled={s.updatingKanbanRecordId === record.id}
												aria-label={`${$t('forms.kanbanMoveTo')} ${fieldLabel(s.kanbanGroupField)}`}
												onchange={(event) =>
													updateKanbanRecordField(record, kanbanField, selectValue(event))}
												class="mt-3 w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
											>
												{#each kanbanMoveOptions(s.kanbanGroupField) as option, index (index)}
													<option value={option}>{option}</option>
												{/each}
											</select>
										{/if}
									</article>
								{/each}
							</div>
						</section>
					{/each}
				</div>
			</section>
		{/each}
	</div>
</div>
