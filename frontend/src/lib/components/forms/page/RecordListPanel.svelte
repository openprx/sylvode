<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { FileText, RefreshCw } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import RecordGrid from '#lib/components/forms/RecordGrid.svelte';
	import ViewBar from '#lib/components/forms/ViewBar.svelte';
	import type { SimpleFilter } from '#lib/components/forms/ViewArrangementControls.svelte';
	import GanttView from '#lib/components/forms/views/GanttView.svelte';
	import PivotView from '#lib/components/forms/views/PivotView.svelte';
	import DetailView from '#lib/components/forms/views/DetailView.svelte';
	import KanbanView from '#lib/components/forms/views/KanbanView.svelte';
	import CalendarView from '#lib/components/forms/views/CalendarView.svelte';
	import CardView from '#lib/components/forms/views/CardView.svelte';
	import TimelineView from '#lib/components/forms/views/TimelineView.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		handleDeleteRecord,
		handleRestoreLastDeletedRecord,
		dismissLastDeletedRecord,
		clearFocusedRecord,
		openRecordEditDrawer
	} = fp.records;
	const {
		goToRecordPage,
		announce,
		recordHref,
		canChangeRecord,
		openRecordPanel,
		closeRecordPanel,
		gridBadges
	} = fp.recordPanel;
	const { memberFieldHidden } = fp.permissions;
	const {
		isServerSortable,
		showView,
		resetArrangement,
		undoReset,
		saveViewAs,
		updateCurrentView,
		deleteView,
		openSaveView,
		openLayoutSettings,
		setDraftFilters
	} = fp.views;
	const { defaultViewColumns, startNewViewDraft } = fp.viewDraft;
</script>

								<section
									class="w-full min-w-0 flex-1 overflow-hidden rounded-card border border-line bg-raised"
									data-record-list-panel
									inert={s.panelMode === 'sheet' && !!s.panelRecordId}
								>
									<ViewBar
										views={s.formViews}
										view={s.savedView}
										currentId={s.selectedViewId}
										currentType={s.viewDraft.view_type}
										modified={s.arrangementModified}
										can={{
											manage:
												!!s.savedView &&
												(s.savedView.config?.visibility === 'private'
													? s.savedView.created_by === s.currentUserId
													: s.canDesign),
											share: s.canDesign,
											setDefault: s.canDesign,
											create: true
										}}
										busy={s.viewSubmitting}
										saveOpen={s.saveViewOpen}
										saveError={s.saveViewError}
										suggestedName={s.saveViewName}
										fields={s.listableFields}
										sortableKeys={s.listableFields
											.map((field) => field.key)
											.filter(isServerSortable)}
										filters={(s.viewDraft.filterGroups[0]?.filters ?? []) as SimpleFilter[]}
										filterLogic={s.viewDraft.filterGroups[0]?.logic ?? 'all'}
										advancedFilters={s.viewDraft.filterGroups.length > 1 ||
											s.viewDraft.filterRootLogic !== 'all'}
										sort={s.viewDraft.sortField
											? { field: s.viewDraft.sortField, direction: s.viewDraft.sortDirection }
											: null}
										columns={s.viewDraft.columns.length > 0
											? s.viewDraft.columns
											: defaultViewColumns()}
										hiddenByPermission={new Set(
											s.listableFields.filter(memberFieldHidden).map((field) => field.key)
										)}
										partialNotice={s.recordQuery.clientFilter &&
										s.serverRecordTotal > s.records.length
											? $t('forms.views.partialFilter', {
													values: { loaded: s.records.length, total: s.serverRecordTotal }
												})
											: null}
										conflict={s.viewConflict}
										undoAvailable={!!s.resetUndo}
										onselect={(id) => showView(id, { remember: true })}
										oncreate={startNewViewDraft}
										onupdate={() => void updateCurrentView()}
										onrename={(name) => void updateCurrentView({ name })}
										onvisibility={(visibility) =>
											void updateCurrentView({
												config: { ...s.savedView?.config, visibility, is_default: false }
											})}
										ondefault={(on) =>
											void updateCurrentView({
												config: { ...s.savedView?.config, is_default: on }
											})}
										onlayout={openLayoutSettings}
										onduplicate={() =>
											openSaveView(
												$t('forms.views.copyName', { values: { name: s.savedView?.name } })
											)}
										onreset={resetArrangement}
										onundo={undoReset}
										onreloadview={() => showView(s.selectedViewId, { remember: false })}
										ondelete={() => void deleteView()}
										onsaveopen={(open) => (open ? openSaveView() : (s.saveViewOpen = false))}
										onsave={(input) => void saveViewAs(input)}
										onfilters={setDraftFilters}
										onsort={(sort) =>
											(s.viewDraft = {
												...s.viewDraft,
												sortField: sort?.field ?? '',
												sortDirection: sort?.direction ?? 'asc'
											})}
										oncolumns={(columns) => (s.viewDraft = { ...s.viewDraft, columns })}
									/>
									{#if s.lastDeletedRecord}
										<div
											class="flex flex-wrap items-center justify-between gap-3 border-b border-line bg-warning-soft px-4 py-2 text-sm text-warning"
											data-record-restore-banner={s.lastDeletedRecord.id}
										>
											<span
												>{$t('forms.recordDeletedRestoreHint', {
													values: { title: s.lastDeletedRecord.title }
												})}</span
											>

											<div class="flex items-center gap-2">
												<Button
													size="sm"
													variant="secondary"
													loading={s.restoringRecordId === s.lastDeletedRecord.id}
													onclick={handleRestoreLastDeletedRecord}
												>
													<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
													{$t('forms.restoreRecord')}
												</Button>

												<Button size="sm" variant="ghost" onclick={dismissLastDeletedRecord}
													>{$t('common.close')}</Button
												>
											</div>
										</div>
									{/if}

									{#if s.focusedRecords.length > 0}
										<div
											class="flex flex-wrap items-center justify-between gap-3 border-b border-line bg-surface px-4 py-2 text-sm text-fg-muted"
											data-record-focus={s.focusedRecords.map((record) => record.id).join('|')}
										>
											<span class="min-w-0 truncate">
												{#if s.focusedRecords.length === 1}
													{$t('forms.focusedRecord')}: {s.focusedRecords[0].title}
												{:else}
													{$t('forms.focusedRecords', {
														values: { count: s.focusedRecords.length }
													})}
												{/if}
											</span>

											<button
												type="button"
												class="rounded-control px-2 py-1 text-xs font-medium text-fg hover:bg-surface"
												onclick={clearFocusedRecord}
												data-record-focus-clear>{$t('forms.clearFocusedRecord')}</button
											>
										</div>
									{/if}

									{#if s.records.length === 0}
										<div class="p-8 text-center text-sm text-fg-muted">
											<FileText class="mx-auto mb-3 h-8 w-8 text-fg-muted" aria-hidden="true" />

											{$t('forms.noRecords')}
										</div>
									{:else if s.viewRecords.length === 0}
										<div class="p-8 text-center text-sm text-fg-muted" data-view-empty-filter>
											<FileText class="mx-auto mb-3 h-8 w-8 text-fg-muted" aria-hidden="true" />

											{$t('forms.noMatchingRecords')}
										</div>
									{:else if s.selectedViewType === 'grid'}
										<RecordGrid
											bind:this={s.recordGrid}
											records={s.paginatedViewRecords}
											fields={s.visibleFields}
											ctx={s.fieldCtx}
											currentId={s.panelRecordId}
											sort={s.viewDraft.sortField
												? { field: s.viewDraft.sortField, direction: s.viewDraft.sortDirection }
												: null}
											sortableKeys={s.visibleFields
												.map((field) => field.key)
												.filter(isServerSortable)}
											page={s.normalizedRecordPage}
											pageCount={s.recordPageCount}
											total={s.viewRecords.length}
											deletingId={s.deletingRecordId}
											{recordHref}
											headerBadges={(field) => gridBadges(field, null)}
											cellBadges={gridBadges}
											canEdit={(record) => canChangeRecord(record, 'record.update')}
											canDelete={(record) => canChangeRecord(record, 'record.delete')}
											onopen={(record, options) =>
												void openRecordPanel(record, options.replace ? 'replace' : 'push')}
											onclosepanel={() => closeRecordPanel()}
											onfocuspanel={() => s.recordPanel?.focusHeading()}
											onedit={openRecordEditDrawer}
											ondelete={(record) => void handleDeleteRecord(record)}
											onsort={(sort) =>
												(s.viewDraft = {
													...s.viewDraft,
													sortField: sort?.field ?? '',
													sortDirection: sort?.direction ?? 'asc'
												})}
											ongotopage={goToRecordPage}
											onannounce={announce}
										/>
									{:else if s.selectedViewType === 'gantt'}
										<GanttView />
									{:else if s.selectedViewType === 'pivot'}
										<PivotView />
									{:else if s.selectedViewType === 'detail'}
										<DetailView />
									{:else if s.selectedViewType === 'kanban'}
										<KanbanView />
									{:else if s.selectedViewType === 'calendar'}
										<CalendarView />
									{:else if s.selectedViewType === 'card'}
										<CardView />
									{:else}
										<TimelineView />
									{/if}
								</section>
