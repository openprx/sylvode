<script lang="ts">
	import ProjectPluginSettingsLink from '#lib/components/plugins/ProjectPluginSettingsLink.svelte';
	import CreatorPluginLink from '#lib/components/plugins/CreatorPluginLink.svelte';
	import FormPluginToolbar from '#lib/components/plugins/FormPluginToolbar.svelte';
	import RecordPluginPanels from '#lib/components/plugins/RecordPluginPanels.svelte';
	import RecordPluginActions from '#lib/components/plugins/RecordPluginActions.svelte';
	import { taskEditorAction } from '#lib/workbench/edit-surface.js';
	import {
		Activity,
		AlertTriangle,
		ArrowDown,
		ArrowLeftRight,
		ArrowUp,
		Calculator,
		ChevronLeft,
		ChevronRight,
		Copy,
		Eye,
		EyeOff,
		FileDown,
		FileText,
		Filter,
		FileUp,
		Link,
		MoreVertical,
		Pencil,
		Plus,
		RefreshCw,
		Shield,
		TableProperties,
		Trash2,
		X
	} from '@lucide/svelte';

	import { afterNavigate } from '$app/navigation';
	import { getContext, onMount, tick, untrack } from 'svelte';
	import { WORKBENCH_CONTEXT, type TaskWorkbench } from '#lib/workbench/tabs.js';
	import { page } from '$app/state';

	import { type FormRecord, type FormView } from '#lib/api/forms.js';

	import Button from '#lib/components/Button.svelte';
	import FormDesignerWorkspace from '#lib/components/forms/FormDesignerWorkspace.svelte';
	import OptionValueBadges from '#lib/components/forms/OptionValueBadges.svelte';
	import LayoutSettingsSheet from '#lib/components/forms/LayoutSettingsSheet.svelte';
	import RecordGrid from '#lib/components/forms/RecordGrid.svelte';
	import RecordSidePanel from '#lib/components/forms/RecordSidePanel.svelte';
	import ViewBar from '#lib/components/forms/ViewBar.svelte';
	import type { SimpleFilter } from '#lib/components/forms/ViewArrangementControls.svelte';
	import { buildDetailSections, headerFields } from '#lib/forms/detail-layout.js';
	import * as arrangement from '#lib/forms/view-arrangement.js';
	import * as precedence from '#lib/forms/view-precedence.js';
	import FormWorkflowTabs from '#lib/components/forms/FormWorkflowTabs.svelte';
	import Modal from '#lib/components/Modal.svelte';

	import { safeLinkUrl } from '#lib/utils/safe-url.js';

	import {
		scenarioTemplateDescription,
		scenarioTemplateIndustry,
		scenarioTemplateName
	} from '#lib/utils/project-type-i18n.js';

	import { t } from 'svelte-i18n';
	import {
		detailLayoutModes,
		fieldTypeOptions,
		formPermissionActions
	} from '#lib/forms/page-constants.js';
	import type {
		CardLayoutMode,
		FormRecordScope,
		GanttScale,
		ViewCalendarDayScope,
		ViewCalendarLayout,
		ViewCalendarRange,
		ViewCalendarRecurrence,
		ViewFilterLogic,
		ViewFilterOperator,
		ViewSortDirection,
		ViewTimelineSource
	} from '#lib/forms/page-types.js';
	import { FormsPage } from '#lib/forms/state/forms-page.js';
	import { setFormsPageContext } from '#lib/forms/state/context.js';
	import {
		attachmentAcceptAttr,
		attachmentThumbnailUrl,
		attachmentIsPreviewableImage,
		signatureImageValue
	} from '#lib/forms/attachment-files.js';
	import {
		calendarScheduleKey,
		calendarDateInputValue,
		calendarRecordIsoDate,
		ganttDateLabel,
		calendarAgendaGroups,
		calendarAgendaRecordCount
	} from '#lib/forms/calendar-dates.js';
	import {
		detailPageWidgetOption,
		detailPageWidgetRegion,
		detailPageFieldColumns,
		keepDetailSectionOpen,
		detailLayoutSectionGridClass,
		detailLayoutSectionFieldClass,
		detailLayoutSectionFieldsForRecord
	} from '#lib/forms/detail-layout-drafts.js';
	import { detailSectionColumns as detailLayoutSectionColumns } from '#lib/forms/detail-layout.js';
	import {
		fieldHasConditionalHidden,
		fieldHasConditionalReadOnly,
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition,
		recordsFieldHiddenByCondition,
		recordsFieldReadOnlyByCondition,
		fieldHiddenByCondition,
		fieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import {
		fieldLabel,
		fieldType,
		fieldOptions,
		optionDisabled,
		optionArchived,
		recordSelectOptions,
		optionDescription,
		optionGroup,
		optionDisplayLabel,
		fieldPlaceholder,
		formListFields,
		formFieldSummary
	} from '#lib/forms/field-options.js';
	import { permissionActionLabel, permissionFieldList } from '#lib/forms/permission-drafts.js';
	import { draftStringFrom, childRelationKeyForField } from '#lib/forms/record-draft-values.js';
	import {
		formsReviewText,
		recordDisplay,
		displayValue,
		formatDate,
		inputValue,
		selectValue
	} from '#lib/forms/record-format.js';
	import {
		isIsoDate,
		viewCalendarRange,
		viewCalendarDayScope,
		viewPivotAggregate,
		viewPivotTotalsMode,
		viewPivotValueDisplay,
		viewPivotSortMode,
		viewPivotRowLimit,
		viewPivotColumnLimit,
		viewCardCoverAspect,
		viewCardCoverFit,
		viewCardCoverPosition,
		cardCoverObjectClass,
		cardCoverImageClass,
		viewCardFieldLimit,
		kanbanMoveOptions
	} from '#lib/forms/view-config.js';
	import {
		viewDraftFilterExpressionRecordIdsText,
		filterLabelSuffix
	} from '#lib/forms/view-draft-parse.js';
	import {
		pivotShowRowTotals,
		pivotShowColumnTotals,
		pivotCellIntensity,
		pivotCellHeatmapStyle,
		formatPivotDisplayValue,
		formatPivotTotalDisplayValue
	} from '#lib/forms/views/pivot-format.js';
	import {
		timelineEventAggregateKeys,
		timelineEventLayout,
		timelineEventAggregateKey,
		timelineEventTitle,
		timelineEventSubtitle,
		timelineEventJson,
		timelineEventDetailsJson
	} from '#lib/forms/views/timeline-events.js';

	import FormsSessionNotice from '#lib/components/forms/page/FormsSessionNotice.svelte';

	import FormsPageHeader from '#lib/components/forms/page/FormsPageHeader.svelte';
	import FormsIndex from '#lib/components/forms/page/FormsIndex.svelte';

	import FormCreatePanel from '#lib/components/forms/page/FormCreatePanel.svelte';

	import FormDataToolbar from '#lib/components/forms/page/FormDataToolbar.svelte';
	import FormConfigHeader from '#lib/components/forms/page/FormConfigHeader.svelte';
	import PermissionStateSummary from '#lib/components/forms/page/PermissionStateSummary.svelte';

	import FormPermissionsMode from '#lib/components/forms/page/FormPermissionsMode.svelte';

	import FormImportExportMode from '#lib/components/forms/page/FormImportExportMode.svelte';
	import ImportWizardModal from '#lib/components/forms/page/ImportWizardModal.svelte';

	import FormDesignMode from '#lib/components/forms/page/FormDesignMode.svelte';
	import DesignerConfirmModal from '#lib/components/forms/page/DesignerConfirmModal.svelte';

	import DetailLayoutMode from '#lib/components/forms/page/DetailLayoutMode.svelte';

	import ViewSetupBasics from '#lib/components/forms/page/ViewSetupBasics.svelte';
	import ViewCardFieldOrder from '#lib/components/forms/page/ViewCardFieldOrder.svelte';
	import ViewKanbanWipPolicy from '#lib/components/forms/page/ViewKanbanWipPolicy.svelte';
	import ViewColumnsDisclosure from '#lib/components/forms/page/ViewColumnsDisclosure.svelte';

	import ViewFilterEditor from '#lib/components/forms/page/ViewFilterEditor.svelte';

	import ViewTypeOptions from '#lib/components/forms/page/ViewTypeOptions.svelte';

	import RecordEditorDrawer from '#lib/components/forms/page/RecordEditorDrawer.svelte';

	import GanttView from '#lib/components/forms/views/GanttView.svelte';
	import DetailView from '#lib/components/forms/views/DetailView.svelte';

	import PivotView from '#lib/components/forms/views/PivotView.svelte';

	import KanbanView from '#lib/components/forms/views/KanbanView.svelte';

	import CalendarView from '#lib/components/forms/views/CalendarView.svelte';

	import CardView from '#lib/components/forms/views/CardView.svelte';

	import TimelineView from '#lib/components/forms/views/TimelineView.svelte';

	// forms-split slot W1-18

	// forms-split slot W2-01

	const workbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const recordEditorAction = taskEditorAction();
	const fp = new FormsPage({ workbench, recordEditorAction });
	setFormsPageContext(fp);
	const s = fp.state;

	const {
		cleanFormsSession,
		formsSessionDirty,
		releaseFormsSession,
		attachFormsSession,
		reloadFormsSession,
		openForm,
		rememberFormsFocus
	} = fp.session;
	const { loadProject, loadScenarioTemplates, loadWorkspaceMembers, loadForms, loadRecords } =
		fp.loaders;
	const {
		handleSaveRecord,
		handleDeleteRecord,
		handleRestoreLastDeletedRecord,
		dismissLastDeletedRecord,
		clearFocusedRecord,
		startEditRecord,
		openRecordCreateDrawer,
		openRecordEditDrawer,
		closeRecordEditorDrawer,
		handleGlobalKeydown,
		handleFormChange,
		handleModeChange,
		relationTargetOptions,
		relationDraftValue,
		setDraftValue,
		isDraftOptionSelected,
		toggleDraftOption,
		draftString,
		optionDisabledForSingleDraft,
		optionDisabledForMultiDraft
	} = fp.records;
	const {
		restoreRecordDetailFromUrl,
		selectRecord,
		goToRecordPage,
		loadSelectedRecordComments,
		reloadSelectedRecord,
		announce,
		recordHref,
		canChangeRecord,
		openRecordPanel,
		closeRecordPanel,
		stepRecordPanel,
		gridBadges
	} = fp.recordPanel;
	const {
		closeRecordDetail,
		handleCreateLink,
		handleCreateRecordComment,
		childFormForRelationField,
		parentChildRelationFields,
		childFieldsForForm,
		startAddChildRecord,
		startEditChildRecord,
		cancelChildEditor,
		saveChildRecord,
		deleteChildRecord,
		selectedRecordAttachmentFields,
		selectedRecordAttachmentValueUrls,
		updateChildDraftValue,
		updateChildTitle,
		isChildDraftOptionSelected,
		toggleChildDraftOption,
		optionDisabledForChildSingleDraft,
		optionDisabledForChildMultiDraft,
		childRecordFields,
		childRecordsForRelationField,
		childGridFieldsForRelationField
	} = fp.recordDetail;
	const {
		attachmentUrl,
		attachmentPreviewUrl,
		fieldAttachments,
		attachmentSignedUrl,
		createAttachmentForField,
		createAttachmentFromLocalFile,
		removeAttachment,
		startSignatureStroke,
		moveSignatureStroke,
		endSignatureStroke,
		clearSignatureCanvas
	} = fp.attachments;
	const {
		handleCreateForm,
		applyScenarioTemplate,
		installScenarioTemplate,
		addFieldDraft,
		removeFieldDraft
	} = fp.createForm;
	const {
		buildSchemaFromDrafts,
		closeDesignerConfirm,
		addDesignerField,
		updateDesignerField,
		duplicateDesignerField,
		removeDesignerField,
		moveDesignerField,
		saveDesignerSchema,
		confirmDesignerSchemaSave
	} = fp.designer;
	const {
		detailLayoutFieldAssignedTo,
		updateDetailLayoutSectionTitle,
		updateDetailLayoutSectionDescription,
		updateDetailLayoutSectionColumns,
		updateDetailLayoutSectionDensity,
		updateDetailLayoutSectionHideEmptyFields,
		updateDetailLayoutSectionCollapsible,
		updateDetailLayoutSectionDefaultCollapsed,
		addDetailLayoutSection,
		removeDetailLayoutSection,
		moveDetailLayoutSection,
		setDetailLayoutSectionField,
		moveDetailLayoutSectionField,
		setDetailHeaderDraft,
		setDetailHighlightField,
		moveDetailHighlightField
	} = fp.detailLayoutEditor;
	const {
		saveDetailHeader,
		saveDetailHighlights,
		saveDetailPageComposition,
		saveDetailLayoutSections,
		setDetailPageWidgetEnabled,
		setDetailPageWidgetTitle,
		setDetailPageWidgetDescription,
		setDetailPageWidgetEmptyText,
		setDetailPageWidgetShowCount,
		setDetailPageWidgetHideEmpty,
		setDetailPageWidgetCollapsible,
		setDetailPageWidgetDefaultCollapsed,
		setDetailPageWidgetDensity,
		setDetailPageWidgetLimit,
		moveDetailPageWidget
	} = fp.detailLayoutSave;
	const {
		saveMemberPermissions,
		setMemberPermission,
		setMemberRecordScope,
		setMemberFieldRead,
		setMemberFieldWrite,
		memberFieldHidden,
		memberFieldLocked
	} = fp.permissions;
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
	const { detailLayoutSections } = fp.viewRecords;
	const {
		defaultViewColumns,
		startNewViewDraft,
		setViewDraftColumn,
		viewDraftHasColumn,
		viewDraftCardFields,
		moveViewDraftCardField
	} = fp.viewDraft;
	const {
		shiftViewDraftCalendarFocus,
		viewDraftSwimlaneFieldCandidates,
		viewDraftCalendarResourceFieldCandidates,
		viewDraftCalendarEndFieldCandidates,
		viewDraftPivotDimensionFieldCandidates,
		viewDraftPivotValueFieldCandidates,
		swapViewDraftPivotAxes,
		viewDraftGanttDateFieldCandidates,
		viewDraftGanttGroupFieldCandidates,
		viewDraftGanttDependencyFieldCandidates,
		viewDraftCalendarExceptionItems,
		addViewDraftCalendarException,
		removeViewDraftCalendarException,
		viewDraftKanbanWipOptions,
		setViewDraftWipLimit
	} = fp.viewDraftFields;
	const {
		updateViewDraftFilterRootLogic,
		updateViewDraftFilterGroupLogic,
		toggleViewDraftFilterGroupDisabled,
		addViewDraftFilterGroup,
		removeViewDraftFilterGroup,
		canDuplicateViewDraftFilterGroup,
		duplicateViewDraftFilterGroup,
		canMergeViewDraftFilterGroup,
		mergeViewDraftFilterGroupWithPrevious,
		moveViewDraftFilterGroup,
		visibleViewDraftFilterGroups,
		hasReachedFilterLimit,
		updateViewDraftFilter,
		toggleViewDraftFilterDisabled,
		addViewDraftFilter,
		removeViewDraftFilter,
		duplicateViewDraftFilter,
		canSplitViewDraftFilterToGroup,
		splitViewDraftFilterToGroup,
		moveViewDraftFilter
	} = fp.viewDraftFilters;
	const {
		viewDraftFilterMatchCount,
		viewDraftFilterGroupMatchCount,
		viewDraftFilterExpressionRecords,
		viewDraftFilterExpressionJsonText,
		viewDraftFilterExpressionSummaryText,
		copyViewDraftFilterExpressionRecordIds,
		copyViewDraftFilterExpressionJson,
		copyViewDraftFilterExpressionSummary,
		copyViewDraftFilterExpressionJsonRecords,
		copyViewDraftFilterExpressionCsvRecords,
		exportViewDraftFilterExpressionConfigJson,
		exportViewDraftFilterExpressionSummary,
		focusViewDraftFilterExpressionRecords,
		exportViewDraftFilterExpressionCsv,
		exportViewDraftFilterExpressionJson
	} = fp.filterExpressionExport;
	const {
		pivotRowRecords,
		pivotColumnRecords,
		pivotCellRecords,
		selectPivotDrilldown,
		pivotDrilldownKey,
		canApplyPivotDrilldownFilters,
		applyPivotDrilldownFilters,
		copyPivotCellValue,
		copyPivotDrilldownRecordIds,
		exportPivotDrilldownRecordsCsv,
		copyPivotTableMatrix,
		exportPivotTableMatrixCsv,
		focusPivotDrilldownRecords
	} = fp.viewPivot;
	const {
		updateKanbanRecordField,
		selectedKanbanRecords,
		isKanbanRecordSelected,
		setKanbanRecordSelected,
		clearKanbanSelection,
		kanbanBulkMoveAllowed,
		bulkMoveKanbanRecords,
		handleKanbanDragStart,
		handleKanbanDragEnd,
		handleKanbanDragOver,
		handleKanbanDrop,
		kanbanColumnRecordCount,
		kanbanWipLabel,
		kanbanColumnWipExceeded,
		kanbanSwimlaneRows
	} = fp.viewKanban;
	const {
		copyCalendarSchedule,
		canShiftCalendarOccurrence,
		shiftCalendarOccurrence,
		updateCalendarRecordDate,
		updateCalendarSeriesDate,
		updateCalendarRecordEndDate,
		handleCalendarDragStart,
		handleCalendarDragEnd,
		handleCalendarDragOver,
		handleCalendarDrop,
		viewCalendarFocusDate,
		calendarResourceRows,
		calendarBucketFilterKey,
		canApplyCalendarBucketFilters,
		applyCalendarBucketFilters
	} = fp.viewCalendar;
	const {
		extendGanttTaskEnd,
		handleGanttDragStart,
		handleGanttDragEnd,
		handleGanttDragOver,
		handleGanttDrop
	} = fp.viewGantt;
	const {
		copyTimelineEventValue,
		exportTimelineEventDetails,
		exportTimelineEventsCsv,
		exportTimelineEventsJson,
		copyTimelineEventIds,
		copyTimelineAggregateIds,
		timelineEventPinned,
		toggleTimelineEventPinned,
		focusTimelineEventRecord,
		timelineEventLinkedRecordIds,
		focusTimelineEventRecords,
		applyTimelineEventQuickFilter,
		applyTimelineEventDayWindow,
		updateTimelineRecordSelectField,
		selectedTimelineSource,
		selectedTimelineEventLayout,
		selectedTimelineEventType,
		selectedTimelineEventActor,
		selectedTimelineEventAggregate,
		selectedTimelineEventQuery,
		selectedTimelineEventFrom,
		selectedTimelineEventTo,
		timelineTitle,
		timelineSubtitle,
		timelineEventRecord
	} = fp.viewTimeline;
	const {
		copyRecordId,
		exportCardRecordDetails,
		exportCardGroupRecords,
		updateCardRecordSelectField,
		cardGroupCollapsed,
		toggleCardGroup,
		cardGroupFilterKey,
		copyCardGroupRecordIds,
		focusCardGroupRecords,
		canApplyCardGroupFilter,
		applyCardGroupFilter,
		cardDisplayFields,
		viewCardBadgeField,
		cardTitle,
		cardSubtitle,
		cardBadge,
		cardCoverUrl
	} = fp.viewCards;
	const {
		exportCurrentView,
		exportCurrentViewJson,
		exportAttachmentManifest,
		exportAttachmentBundle,
		exportAttachmentPackage
	} = fp.exporter;
	const {
		openImportRecords,
		closeImportRecords,
		resetImportPreviewForSourceChange,
		continueImportFromSource,
		continueImportFromMapping,
		exportImportTemplate,
		loadImportFile,
		refreshImportMappingFromText,
		saveImportMappingTemplate,
		restoreImportMappingTemplate,
		applyImportMapping,
		commitImportRecords
	} = fp.importer;

	$effect(() => {
		if (s.formsSessionKey) workbench?.markDirty(s.formsSessionKey, formsSessionDirty());
	});
	onMount(() => {
		const unsubscribe = workbench?.subscribe(() => {
			if (s.formsSessionReady && !s.formsSessionActivating) void attachFormsSession();
		});
		return () => {
			unsubscribe?.();
			s.formsSessionLatch.dispose();
			releaseFormsSession();
		};
	});

	$effect(() => {
		const formId = page.url.searchParams.get('form') ?? page.url.searchParams.get('form_id');
		if (
			s.formsSessionReady &&
			formId &&
			formId !== s.selectedFormId &&
			formId !== s.formsSessionActivating
		)
			void openForm(formId, { syncUrl: false });
	});

	onMount(async () => {
		await Promise.all([
			loadProject(),
			loadForms(),
			loadScenarioTemplates(),
			loadWorkspaceMembers()
		]);

		await restoreRecordDetailFromUrl();
		s.loading = false;
		if (!s.formsSessionKey) cleanFormsSession();
		s.formsSessionReady = true;
		await attachFormsSession();
	});

	$effect(() => {
		// Editor closed (saved or cancelled) while a record was open: back to the panel's Edit.
		if (!s.recordEditorOpen && untrack(() => s.panelRecordId))
			void tick().then(() =>
				document.querySelector<HTMLElement>('[data-record-panel-edit]')?.focus()
			);
	});

	$effect(() => {
		const view = s.baseView;
		const [modified, config, type] = [
			s.arrangementModified,
			s.draftViewConfig,
			s.viewDraft.view_type
		];
		const [user, formId] = [s.currentUserId, s.selectedFormId];
		if (!view || !formId || untrack(() => s.viewBaseline.viewId) !== view.id) return;
		clearTimeout(s.arrangementTimer);
		s.arrangementTimer = setTimeout(() => {
			const base = untrack(() => s.savedView?.updated_at ?? null);
			if (!modified) arrangement.clearArrangement(user, formId, view.id);
			else
				arrangement.writeArrangement(user, formId, view.id, {
					base_updated_at: base,
					view_type: type,
					config,
					at: new Date().toISOString()
				});
		}, 300);
	});

	$effect(() => {
		const key = JSON.stringify([s.selectedFormId, s.recordQuery]);
		clearTimeout(s.recordQueryTimer);
		if (!untrack(() => s.loadedRecordQuery) || key === untrack(() => s.loadedRecordQuery)) return;
		s.recordQueryTimer = setTimeout(() => void loadRecords(), 300);
	});

	afterNavigate(() => {
		const id = s.pendingRowFocus;
		s.pendingRowFocus = null;
		if (id && !s.panelRecordId && document.activeElement === document.body)
			s.recordGrid?.focusRow(id);
	});

	// Back / Forward open and close the panel; the panel's own writes never reach this branch.
	$effect(() => {
		const id = s.detailUrlRecordId;
		if (!s.formsSessionReady || s.formsSessionActivating || !s.selectedFormId) return;
		untrack(() => {
			if (id && id !== s.panelRecordId) {
				s.panelHistoryDepth = Math.max(0, s.panelHistoryDepth - 1);
				void openRecordPanel(id, 'none');
			} else if (!id && s.panelRecordId) {
				s.panelHistoryDepth = 0;
				const closing = s.panelRecordId;
				s.panelRecordId = null;
				s.selectedRecord = null;
				void tick().then(() => s.recordGrid?.focusRow(closing));
			}
		});
	});
</script>

<svelte:window onkeydown={handleGlobalKeydown} />
<div class="sr-only" role="status" aria-live="polite" data-record-live-region>{s.liveMessage}</div>

<div
	class="mx-auto max-w-7xl space-y-5"
	bind:this={s.formsSessionRoot}
	onfocusout={rememberFormsFocus}
	aria-busy={s.formsSessionTransitioning}
	inert={s.formsSessionTransitioning}
>
	{#if !s.recordEditorOpen}<FormsSessionNotice />{/if}
	<FormsPageHeader />

	{#if s.loading}
		<div class="rounded-card border border-line bg-raised p-6 text-sm text-fg-muted">
			{$t('forms.loadingForms')}
		</div>
	{:else}
		{#if s.showCreateForm || s.forms.length === 0}
			<FormCreatePanel />
		{/if}

		{#if s.forms.length > 0 && !s.selectedForm}
			<FormsIndex />
		{/if}

		{#if s.forms.length > 0 && s.selectedForm}
			<section class="space-y-4">
				{#if s.activeMode === 'data'}
					<FormDataToolbar />
				{:else}
					<FormConfigHeader />
				{/if}

				{#if s.formPermissions && s.activeMode !== 'permissions' && s.activeMode !== 'data'}
					<PermissionStateSummary />
				{/if}

				{#if s.activeMode === 'permissions'}
					<FormPermissionsMode />
				{:else if s.activeMode === 'importExport'}
					<FormImportExportMode />
				{:else if s.activeMode === 'design' || s.activeMode === 'layout'}
					<div
						class="grid min-w-0 gap-4"
						data-form-mode={s.activeMode === 'design' ? 'field-design' : 'detail-layout'}
					>
						{#if s.activeMode === 'design'}
							<FormDesignMode />
						{/if}

						{#if s.activeMode === 'layout'}
							<DetailLayoutMode />
						{/if}
					</div>
				{:else}
					<div class="grid min-w-0 gap-4">
						{#if s.activeMode === 'data'}
							<div class="flex min-w-0 items-start gap-4" data-record-list-stack>
								<LayoutSettingsSheet
									open={s.viewToolsExpanded}
									viewName={s.selectedView?.name ?? ''}
									onclose={() => (s.viewToolsExpanded = false)}
								>
									<section class="w-full" data-view-tools-expanded-panel>
										<div class="space-y-4">
											<div class="rounded-control border border-line bg-raised" data-view-setup>
												<div class="space-y-2 border-t border-line p-3">
													<ViewSetupBasics />

													<div
														class="space-y-3 rounded-card border border-line bg-nav p-3"
														data-view-config-panel
													>
														<div>
															<h4 class="text-sm font-semibold text-fg">
																{$t('forms.viewRules')}
															</h4>

															<p class="mt-1 text-xs text-fg-muted">
																{$t('forms.viewRulesDescription')}
															</p>
														</div>

														<ViewFilterEditor />

														<ViewTypeOptions />

														{#if s.viewDraft.view_type === 'card' && viewDraftCardFields().length > 0}
															<ViewCardFieldOrder />
														{/if}

														{#if viewDraftKanbanWipOptions().length > 0}
															<ViewKanbanWipPolicy />
														{/if}
													</div>
												</div>
											</div>

												<ViewColumnsDisclosure />
										</div>
									</section>
								</LayoutSettingsSheet>

								{#if s.recordEditorOpen}
									<RecordEditorDrawer />
								{/if}

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
								{#if s.panelRecordId && s.selectedForm}
									<RecordSidePanel
										bind:this={s.recordPanel}
										bind:appsOpen={s.panelAppsOpen}
										record={s.panelRecord}
										status={s.panelStatus}
										errorMessage={s.panelError}
										inView={s.panelPosition >= 0}
										form={s.selectedForm}
										sections={buildDetailSections(s.selectedForm, s.fields, {
											main: $t('forms.detailSectionMain'),
											other: $t('forms.recordPanel.otherFields')
										})}
										header={headerFields(s.selectedForm, s.fields)}
										ctx={s.fieldCtx}
										childTable={(field) => ({
											rows: childRecordsForRelationField(field),
											columns: childGridFieldsForRelationField(field)
										})}
										detailsLoading={s.panelDetailsLoading}
										position={s.panelPosition >= 0
											? { index: s.panelPosition + 1, total: s.viewRecords.length }
											: null}
										fullPageHref={recordHref(s.panelRecordId)}
										{recordHref}
										recordUrl={new URL(page.url.href).href}
										memberName={(id) =>
											s.workspaceMembers.find((member) => member.user_id === id)?.name ??
											$t('forms.fieldValue.unknownMember')}
										canEdit={!!s.panelRecord && canChangeRecord(s.panelRecord, 'record.update')}
										canDelete={!!s.panelRecord && canChangeRecord(s.panelRecord, 'record.delete')}
										canRestore={s.lastDeletedRecord?.id === s.panelRecordId}
										showConfigBadges={s.canDesign}
										configBadges={(field) =>
											[
												memberFieldHidden(field) && $t('forms.recordPanel.configHidden'),
												memberFieldLocked(field) && $t('forms.recordPanel.configLocked'),
												(fieldHasConditionalHidden(field) || fieldHasConditionalReadOnly(field)) &&
													$t('forms.recordPanel.configConditional')
											].filter((label): label is string => !!label)}
										isHiddenByCondition={recordFieldHiddenByCondition}
										plugins={{
											workspaceId: s.workspaceId,
											projectId: s.projectId,
											surface: s.recordPluginActions,
											enabled: s.selectedForm.project_id === s.projectId
										}}
										width={s.panelWidth}
										inert={s.recordEditorOpen}
										onclose={() => closeRecordPanel()}
										onprev={() => stepRecordPanel(-1)}
										onnext={() => stepRecordPanel(1)}
										onedit={() => s.panelRecord && openRecordEditDrawer(s.panelRecord)}
										ondelete={() => s.panelRecord && void handleDeleteRecord(s.panelRecord)}
										onrestore={() => void handleRestoreLastDeletedRecord()}
										onretry={() => s.panelRecordId && void openRecordPanel(s.panelRecordId, 'none')}
										onshowall={() => showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false })}
										onresize={(px) => {
											s.panelWidth = px;
											arrangement.writePanelWidth(s.currentUserId, px);
										}}
										onopenrelation={(id, sameForm, event) => {
											if (!sameForm) return;
											event.preventDefault();
											void openRecordPanel(id, 'push');
										}}
										onpluginapplied={() => void reloadSelectedRecord()}
										onfocuslist={() => s.recordGrid?.focusRow(s.panelRecordId)}
										onannounce={announce}
										onmodechange={(mode) => (s.panelMode = mode)}
										onmoredetails={() => {
											s.activeMode = 'detail';
											void loadSelectedRecordComments();
										}}
									/>
								{/if}
							</div>
						{/if}

						{#if s.activeMode === 'detail'}
							<aside
								class="rounded-card border border-line bg-raised"
								data-record-detail-drawer={s.selectedRecord?.id ?? ''}
								data-record-detail-url-record={s.detailUrlRecordId}
							>
								{#if s.selectedRecord}
									<div class="border-b border-line p-4">
										<div class="flex items-start justify-between gap-3">
											<div class="min-w-0">
												<h3 class="truncate text-base font-semibold text-fg">
													{s.selectedRecord.title}
												</h3>

												<p class="mt-1 font-mono text-xs text-fg-muted">
													{s.selectedRecord.id}
												</p>

												<p class="mt-2 text-xs text-fg-muted">
													{$t('forms.updatedAt', {
														values: { date: formatDate(s.selectedRecord.updated_at) }
													})}
												</p>
											</div>

											<div class="flex flex-wrap justify-end gap-2">
												{#if s.selectedForm?.key === 'order_line'}
													<CreatorPluginLink
														workspaceId={s.workspaceId}
														projectId={s.projectId}
														formKey={s.selectedForm.key}
														slot="form.record.actions"
													/>
												{/if}
												<RecordPluginActions
													recordId={s.selectedRecord.id}
													surface={s.recordPluginActions}
												/>
												<Button
													size="sm"
													variant="secondary"
													onclick={() => {
														if (s.selectedRecord) startEditRecord(s.selectedRecord);
													}}
												>
													<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
													{$t('forms.edit')}
												</Button>

												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface hover:text-fg"
													aria-label={$t('common.close')}
													onclick={closeRecordDetail}
													data-record-detail-close={s.selectedRecord.id}
													><X class="h-4 w-4" aria-hidden="true" /></button
												>
											</div>
										</div>
									</div>

									{#if s.selectedRecord.project_id === s.projectId}
										{#if s.selectedForm?.key === 'order_line'}
											<CreatorPluginLink
												workspaceId={s.workspaceId}
												projectId={s.projectId}
												formKey={s.selectedForm.key}
												slot="form.record.detail"
											/>
										{/if}
										{#key s.selectedRecord.id}
											<RecordPluginPanels
												recordId={s.selectedRecord.id}
												surface={s.recordPluginActions}
												onApplied={() => void reloadSelectedRecord()}
											/>
										{/key}
									{/if}

									<div class="divide-y divide-line" data-record-detail-layout={s.selectedRecord.id}>
										{#each detailLayoutSections() as section}
											<details
												class={section.density === 'compact' ? 'px-4 py-3' : 'px-4 py-4'}
												open={!section.collapsible || !section.defaultCollapsed}
												ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
												data-record-detail-section={`${s.selectedRecord.id}:${section.key}`}
												data-record-detail-section-density={`${s.selectedRecord.id}:${section.key}:${section.density}`}
												data-record-detail-section-collapsible={`${s.selectedRecord.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
												data-record-detail-section-open={`${s.selectedRecord.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
											>
												<summary
													class={section.collapsible
														? 'cursor-pointer list-none'
														: 'cursor-default list-none'}
												>
													<h4 class="mb-3 inline text-xs font-semibold uppercase text-fg-muted">
														{section.title}
													</h4>
												</summary>

												{#if section.description}
													<p
														class="mb-3 text-sm text-fg-muted"
														data-record-detail-section-description={`${s.selectedRecord.id}:${section.key}`}
													>
														{section.description}
													</p>
												{/if}

												<dl
													class={detailLayoutSectionGridClass(section.columns, section.density)}
													data-record-detail-section-columns={`${s.selectedRecord.id}:${section.key}:${section.columns}`}
												>
													{#each detailLayoutSectionFieldsForRecord(section, s.selectedRecord) as field}
														<div
															class={detailLayoutSectionFieldClass(field, section.columns)}
															data-record-detail-field={`${s.selectedRecord.id}:${section.key}:${field.key}`}
														>
															<dt
																class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted"
															>
																<span class="min-w-0 truncate">{fieldLabel(field)}</span>

																{#if memberFieldHidden(field)}
																	<span
																		class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
																		data-record-detail-permission-hidden={field.key}
																		>{$t('forms.fieldHiddenBadge')}</span
																	>
																{/if}

																{#if memberFieldLocked(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-record-detail-permission-locked={field.key}
																		>{$t('forms.fieldLockedBadge')}</span
																	>
																{/if}

																{#if recordFieldHiddenByCondition(field, s.selectedRecord)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-record-detail-conditional-hidden={field.key}
																		>{$t('forms.fieldConditionHiddenBadge')}</span
																	>
																{/if}

																{#if recordFieldReadOnlyByCondition(field, s.selectedRecord)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-record-detail-conditional-readonly={field.key}
																		>{$t('forms.fieldConditionReadOnlyBadge')}</span
																	>
																{/if}
															</dt>

															<dd class="mt-1 break-words text-sm text-fg">
																{#if fieldType(field) === 'child_table'}
																	{@const childRecords = childRecordsForRelationField(field)}
																	{@const childFields = childGridFieldsForRelationField(field)}
																	{@const childForm = childFormForRelationField(field)}

																	<div
																		class="overflow-hidden rounded-control border border-line"
																		data-record-detail-child-table-widget={`${s.selectedRecord.id}:${section.key}:${field.key}`}
																		data-record-detail-child-table-relation-key={childRelationKeyForField(
																			field
																		)}
																		data-record-detail-child-table-child-form={childForm?.key}
																	>
																		<div
																			class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
																		>
																			<div class="min-w-0">
																				<div
																					class="truncate text-xs font-semibold uppercase text-fg-muted"
																					data-record-detail-child-table-title={`${s.selectedRecord.id}:${field.key}`}
																				>
																					{fieldLabel(field)}
																				</div>

																				<div
																					class="mt-0.5 text-[11px] text-fg-muted"
																					data-record-detail-child-table-meta={`${s.selectedRecord.id}:${field.key}`}
																				>
																					{childForm?.name ?? childRelationKeyForField(field)} · {childRelationKeyForField(
																						field
																					)}
																				</div>
																			</div>

																			<span
																				class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
																				data-record-detail-child-table-count={`${field.key}:${childRecords.length}`}
																				>{$t('forms.groupRecordCount', {
																					values: { count: childRecords.length }
																				})}</span
																			>
																		</div>

																		{#if childRecords.length > 0}
																			<div class="overflow-x-auto">
																				<table
																					class="min-w-full divide-y divide-line text-xs"
																					data-record-detail-child-table-grid={field.key}
																				>
																					<thead class="bg-nav">
																						<tr>
																							<th
																								class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
																								>{$t('forms.recordTitle')}</th
																							>

																							{#each childFields as childField}
																								<th
																									class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
																									data-record-detail-child-table-column={`${field.key}:${childField.key}`}
																									>{fieldLabel(childField)}</th
																								>
																							{/each}

																							<th
																								class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
																								>{$t('common.actions')}</th
																							>
																						</tr>
																					</thead>

																					<tbody class="divide-y divide-line">
																						{#each childRecords as child}
																							<tr
																								data-record-detail-child-table-row={`${field.key}:${child.record.id}`}
																							>
																								<td class="px-3 py-2 font-medium text-fg"
																									>{child.record.title}</td
																								>

																								{#each childFields as childField}
																									<td
																										class="max-w-56 px-3 py-2 text-fg"
																										data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
																										><span class="line-clamp-2 break-words"
																											>{displayValue(
																												child.record.values[childField.key]
																											)}</span
																										></td
																									>
																								{/each}

																								<td class="px-3 py-2 text-right">
																									<div class="flex justify-end gap-1">
																										<button
																											type="button"
																											class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																											onclick={() => startEditChildRecord(child)}
																											aria-label={$t('forms.editChildRecord')}
																											data-record-detail-child-table-edit-row={`${field.key}:${child.record.id}`}
																											><Pencil
																												class="h-4 w-4"
																												aria-hidden="true"
																											/></button
																										>

																										<button
																											type="button"
																											class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																											disabled={s.deletingChildRecordId ===
																												child.record.id}
																											onclick={() => deleteChildRecord(child)}
																											aria-label={$t('forms.deleteChildRecord')}
																											data-record-detail-child-table-delete-row={`${field.key}:${child.record.id}`}
																											><Trash2
																												class="h-4 w-4"
																												aria-hidden="true"
																											/></button
																										>
																									</div>
																								</td>
																							</tr>
																						{/each}
																					</tbody>
																				</table>
																			</div>
																		{:else}
																			<p
																				class="px-3 py-3 text-sm text-fg-muted"
																				data-record-detail-child-table-empty={field.key}
																			>
																				{$t('forms.noChildRecords')}
																			</p>
																		{/if}
																	</div>
																{:else}
																	<OptionValueBadges
																		{field}
																		value={s.selectedRecord.values[field.key]}
																	/>
																{/if}
															</dd>
														</div>
													{/each}
												</dl>
											</details>
										{/each}

										{#if parentChildRelationFields().length > 0}
											<section
												class="px-4 py-4"
												data-record-detail-section={`${s.selectedRecord.id}:child_tables`}
											>
												<h4 class="mb-3 text-xs font-semibold uppercase text-fg-muted">
													{$t('forms.childRecords')}
												</h4>

												<div class="space-y-3" data-record-detail-child-table-widgets>
													{#each parentChildRelationFields() as relationField}
														{@const childRecords = childRecordsForRelationField(relationField)}
														{@const childFields = childGridFieldsForRelationField(relationField)}
														{@const childForm = childFormForRelationField(relationField)}

														<div
															class="overflow-hidden rounded-control border border-line"
															data-record-detail-child-table-widget={`${s.selectedRecord.id}:child_tables:${relationField.key}`}
															data-record-detail-child-table-relation-key={childRelationKeyForField(
																relationField
															)}
															data-record-detail-child-table-child-form={childForm?.key}
														>
															<div
																class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
															>
																<div class="min-w-0">
																	<div
																		class="truncate text-xs font-semibold uppercase text-fg-muted"
																		data-record-detail-child-table-title={`${s.selectedRecord.id}:${relationField.key}`}
																	>
																		{fieldLabel(relationField)}
																	</div>

																	<div
																		class="mt-0.5 text-[11px] text-fg-muted"
																		data-record-detail-child-table-meta={`${s.selectedRecord.id}:${relationField.key}`}
																	>
																		{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(
																			relationField
																		)}
																	</div>
																</div>

																<span
																	class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
																	data-record-detail-child-table-count={`${relationField.key}:${childRecords.length}`}
																	>{$t('forms.groupRecordCount', {
																		values: { count: childRecords.length }
																	})}</span
																>
															</div>

															{#if childRecords.length > 0}
																<div class="overflow-x-auto">
																	<table
																		class="min-w-full divide-y divide-line text-xs"
																		data-record-detail-child-table-grid={relationField.key}
																	>
																		<thead class="bg-nav">
																			<tr>
																				<th
																					class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
																					>{$t('forms.recordTitle')}</th
																				>

																				{#each childFields as childField}
																					<th
																						class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
																						data-record-detail-child-table-column={`${relationField.key}:${childField.key}`}
																						>{fieldLabel(childField)}</th
																					>
																				{/each}

																				<th
																					class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
																					>{$t('common.actions')}</th
																				>
																			</tr>
																		</thead>

																		<tbody class="divide-y divide-line">
																			{#each childRecords as child}
																				<tr
																					data-record-detail-child-table-row={`${relationField.key}:${child.record.id}`}
																				>
																					<td class="px-3 py-2 font-medium text-fg"
																						>{child.record.title}</td
																					>

																					{#each childFields as childField}
																						<td
																							class="max-w-56 px-3 py-2 text-fg"
																							data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
																							><span class="line-clamp-2 break-words"
																								>{displayValue(
																									child.record.values[childField.key]
																								)}</span
																							></td
																						>
																					{/each}

																					<td class="px-3 py-2 text-right">
																						<div class="flex justify-end gap-1">
																							<button
																								type="button"
																								class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																								onclick={() => startEditChildRecord(child)}
																								aria-label={$t('forms.editChildRecord')}
																								data-record-detail-child-table-edit-row={`${relationField.key}:${child.record.id}`}
																								><Pencil
																									class="h-4 w-4"
																									aria-hidden="true"
																								/></button
																							>

																							<button
																								type="button"
																								class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																								disabled={s.deletingChildRecordId ===
																									child.record.id}
																								onclick={() => deleteChildRecord(child)}
																								aria-label={$t('forms.deleteChildRecord')}
																								data-record-detail-child-table-delete-row={`${relationField.key}:${child.record.id}`}
																								><Trash2
																									class="h-4 w-4"
																									aria-hidden="true"
																								/></button
																							>
																						</div>
																					</td>
																				</tr>
																			{/each}
																		</tbody>
																	</table>
																</div>
															{:else}
																<p
																	class="px-3 py-3 text-sm text-fg-muted"
																	data-record-detail-child-table-empty={relationField.key}
																>
																	{$t('forms.noChildRecords')}
																</p>
															{/if}
														</div>
													{/each}
												</div>
											</section>
										{/if}
									</div>

									<div class="border-t border-line p-4">
										<div class="mb-5">
											<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
												<div class="flex items-center gap-2">
													<TableProperties class="h-4 w-4 text-fg-muted" aria-hidden="true" />

													<h4 class="text-sm font-semibold text-fg">
														{$t('forms.childRecords')}
													</h4>
												</div>

												<div class="flex flex-wrap items-center gap-2">
													{#each parentChildRelationFields() as relationField}
														<Button
															size="sm"
															variant="secondary"
															onclick={() => startAddChildRecord(relationField)}
														>
															<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.addChildRecord')} · {fieldLabel(relationField)}
														</Button>
													{/each}
												</div>
											</div>

											{#if s.childEditor}
												<div class="mb-3 rounded-card border border-line bg-surface p-3">
													<div class="mb-3 flex items-center justify-between gap-3">
														<h5 class="text-sm font-semibold text-fg">
															{s.childEditor.recordId
																? $t('forms.editChildRecord')
																: $t('forms.addChildRecord')}
														</h5>

														<Button size="sm" variant="secondary" onclick={cancelChildEditor}>
															<X class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('common.cancel')}
														</Button>
													</div>

													<div class="grid gap-3 md:grid-cols-2">
														<label class="space-y-1 text-sm">
															<span class="font-medium text-fg">{$t('forms.titleOverride')}</span>

															<input
																class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																value={s.childEditor.title}
																oninput={(event) => updateChildTitle(inputValue(event))}
															/>
														</label>

														{#each childFieldsForForm(s.childEditor.childFormId) as childField}
															<label
																class="space-y-1 text-sm {fieldHiddenByCondition(
																	childField,
																	s.childEditor.draft
																)
																	? 'hidden'
																	: ''}"
																data-child-record-edit-field={childField.key}
																data-conditional-hidden={fieldHiddenByCondition(
																	childField,
																	s.childEditor.draft
																)
																	? childField.key
																	: undefined}
																data-conditional-readonly={fieldReadOnlyByCondition(
																	childField,
																	s.childEditor.draft
																)
																	? childField.key
																	: undefined}
															>
																<span class="font-medium text-fg">
																	{fieldLabel(childField)}

																	{#if childField.required}
																		<span class="text-danger">*</span>
																	{/if}
																</span>

																{#if fieldType(childField) === 'boolean'}
																	<div
																		class="flex min-h-10 items-center rounded-card border border-input bg-field px-3 py-2"
																	>
																		<input
																			type="checkbox"
																			class="h-4 w-4 rounded border-input accent-primary"
																			checked={s.childEditor.draft[childField.key] === true}
																			disabled={fieldReadOnlyByCondition(
																				childField,
																				s.childEditor.draft
																			)}
																			onchange={(event) =>
																				updateChildDraftValue(
																					childField.key,
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																				)}
																		/>
																	</div>
																{:else if fieldType(childField) === 'single_select' && fieldOptions(childField).length > 0}
																	<select
																		value={draftStringFrom(s.childEditor.draft, childField.key)}
																		disabled={fieldReadOnlyByCondition(
																			childField,
																			s.childEditor.draft
																		)}
																		onchange={(event) =>
																			updateChildDraftValue(childField.key, selectValue(event))}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																	>
																		<option value="">{$t('forms.selectPlaceholder')}</option>

																		{#each recordSelectOptions(childField, s.childEditor.draft) as option}
																			<option
																				value={option}
																				disabled={optionDisabledForChildSingleDraft(
																					childField,
																					option
																				)}
																				data-option-disabled={optionDisabled(childField, option)
																					? option
																					: undefined}
																				data-option-archived={optionArchived(childField, option)
																					? option
																					: undefined}
																				data-option-description={optionDescription(
																					childField,
																					option
																				) || undefined}
																				data-option-group={optionGroup(childField, option) ||
																					undefined}
																				>{optionDisplayLabel(childField, option)}</option
																			>
																		{/each}
																	</select>
																{:else if fieldType(childField) === 'multi_select' && fieldOptions(childField).length > 0}
																	<div
																		class="grid gap-2 rounded-control border border-input bg-field p-2 text-sm"
																	>
																		{#each recordSelectOptions(childField, s.childEditor.draft) as option}
																			<label
																				class="flex items-start gap-2 text-fg"
																				data-option-description={optionDescription(
																					childField,
																					option
																				) || undefined}
																				data-option-group={optionGroup(childField, option) ||
																					undefined}
																			>
																				<input
																					type="checkbox"
																					class="h-4 w-4 rounded border-input accent-primary"
																					checked={isChildDraftOptionSelected(
																						childField.key,
																						option
																					)}
																					disabled={fieldReadOnlyByCondition(
																						childField,
																						s.childEditor.draft
																					) || optionDisabledForChildMultiDraft(childField, option)}
																					data-option-disabled={optionDisabled(childField, option)
																						? option
																						: undefined}
																					data-option-archived={optionArchived(childField, option)
																						? option
																						: undefined}
																					onchange={(event) =>
																						toggleChildDraftOption(
																							childField.key,
																							option,
																							event.currentTarget instanceof HTMLInputElement
																								? event.currentTarget.checked
																								: false
																						)}
																				/>

																				<span class="min-w-0">
																					<span class="block"
																						>{optionDisplayLabel(childField, option)}</span
																					>

																					{#if optionDescription(childField, option)}
																						<span class="block text-xs text-fg-muted"
																							>{optionDescription(childField, option)}</span
																						>
																					{/if}
																				</span>
																			</label>
																		{/each}
																	</div>
																{:else if fieldType(childField) === 'member'}
																	<select
																		value={draftStringFrom(s.childEditor.draft, childField.key)}
																		disabled={fieldReadOnlyByCondition(
																			childField,
																			s.childEditor.draft
																		) || s.workspaceMembersLoading}
																		onchange={(event) =>
																			updateChildDraftValue(childField.key, selectValue(event))}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																	>
																		<option value="">
																			{s.workspaceMembersLoading
																				? $t('forms.loadingMembers')
																				: $t('forms.selectMemberPlaceholder')}
																		</option>

																		{#each s.workspaceMembers as member (member.user_id)}
																			<option value={member.user_id}
																				>{member.name || member.email || member.user_id}</option
																			>
																		{/each}
																	</select>
																{:else if fieldType(childField) === 'textarea' || fieldType(childField) === 'rich_text'}
																	<textarea
																		class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																		value={draftStringFrom(s.childEditor.draft, childField.key)}
																		placeholder={fieldPlaceholder(childField)}
																		readonly={fieldReadOnlyByCondition(
																			childField,
																			s.childEditor.draft
																		)}
																		maxlength={childField.validation?.max_length}
																		oninput={(event) =>
																			updateChildDraftValue(childField.key, inputValue(event))}
																	></textarea>
																{:else if fieldType(childField) === 'signature'}
																	<div
																		class="space-y-2"
																		data-child-signature-field={childField.key}
																	>
																		<canvas
																			class="h-24 w-full touch-none rounded-card border border-dashed border-input bg-white"
																			data-child-signature-canvas={childField.key}
																			aria-label={$t('forms.signatureCanvas')}
																			onpointerdown={(event) =>
																				startSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					s.childEditor
																						? fieldReadOnlyByCondition(
																								childField,
																								s.childEditor.draft
																							)
																						: true
																				)}
																			onpointermove={(event) =>
																				moveSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					s.childEditor
																						? fieldReadOnlyByCondition(
																								childField,
																								s.childEditor.draft
																							)
																						: true
																				)}
																			onpointerup={(event) =>
																				endSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					s.childEditor
																						? fieldReadOnlyByCondition(
																								childField,
																								s.childEditor.draft
																							)
																						: true
																				)}
																			onpointercancel={(event) =>
																				endSignatureStroke(
																					event,
																					childField.key,
																					'child',
																					s.childEditor
																						? fieldReadOnlyByCondition(
																								childField,
																								s.childEditor.draft
																							)
																						: true
																				)}
																		></canvas>

																		<div class="flex flex-wrap items-center gap-2">
																			<Button
																				size="sm"
																				variant="secondary"
																				disabled={fieldReadOnlyByCondition(
																					childField,
																					s.childEditor.draft
																				)}
																				onclick={() =>
																					clearSignatureCanvas(
																						childField.key,
																						'child',
																						s.childEditor
																							? fieldReadOnlyByCondition(
																									childField,
																									s.childEditor.draft
																								)
																							: true
																					)}>{$t('forms.signatureClear')}</Button
																			>

																			{#if signatureImageValue(s.childEditor.draft, childField.key)}
																				<img
																					src={signatureImageValue(
																						s.childEditor.draft,
																						childField.key
																					)}
																					alt={fieldLabel(childField)}
																					class="h-10 max-w-48 rounded border border-line bg-raised object-contain"
																					data-child-signature-preview={childField.key}
																				/>
																			{/if}
																		</div>

																		<textarea
																			class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
																			value={draftStringFrom(s.childEditor.draft, childField.key)}
																			placeholder={fieldPlaceholder(childField)}
																			readonly={fieldReadOnlyByCondition(
																				childField,
																				s.childEditor.draft
																			)}
																			maxlength={childField.validation?.max_length}
																			oninput={(event) =>
																				updateChildDraftValue(childField.key, inputValue(event))}
																			data-child-signature-value={childField.key}></textarea>
																	</div>
																{:else if fieldType(childField) === 'autonumber'}
																	<input
																		type="text"
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg-muted"
																		value={draftStringFrom(s.childEditor.draft, childField.key)}
																		placeholder={$t('forms.autonumberGeneratedAfterSave')}
																		readonly
																		data-child-autonumber-field={childField.key}
																	/>
																{:else if fieldType(childField) === 'child_table'}
																	<div
																		class="rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg-muted"
																		data-child-table-field={childField.key}
																	>
																		{$t('forms.childRecords')}
																	</div>
																{:else}
																	<input
																		type={fieldType(childField) === 'date'
																			? 'date'
																			: fieldType(childField) === 'datetime'
																				? 'datetime-local'
																				: fieldType(childField) === 'email'
																					? 'email'
																					: fieldType(childField) === 'phone'
																						? 'tel'
																						: [
																									'amount',
																									'number',
																									'integer',
																									'rating',
																									'progress'
																							  ].includes(fieldType(childField))
																							? 'number'
																							: 'text'}
																		step={['integer', 'rating', 'progress'].includes(
																			fieldType(childField)
																		)
																			? '1'
																			: ['amount', 'number'].includes(fieldType(childField))
																				? '0.01'
																				: undefined}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																		value={draftStringFrom(s.childEditor.draft, childField.key)}
																		placeholder={fieldPlaceholder(childField)}
																		readonly={fieldReadOnlyByCondition(
																			childField,
																			s.childEditor.draft
																		)}
																		min={childField.validation?.min}
																		max={childField.validation?.max}
																		pattern={childField.validation?.pattern}
																		maxlength={childField.validation?.max_length}
																		oninput={(event) =>
																			updateChildDraftValue(childField.key, inputValue(event))}
																	/>
																{/if}

																{#if childField.help_text}
																	<span class="block text-xs text-fg-muted"
																		>{childField.help_text}</span
																	>
																{/if}
															</label>
														{/each}
													</div>

													<div class="mt-3 flex justify-end gap-2">
														<Button size="sm" variant="secondary" onclick={cancelChildEditor}
															>{$t('common.cancel')}</Button
														>

														<Button size="sm" loading={s.childSubmitting} onclick={saveChildRecord}>
															<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
															{$t('forms.saveRecord')}
														</Button>
													</div>
												</div>
											{/if}

											{#if parentChildRelationFields().length > 0}
												<div class="space-y-3" data-child-table-grids>
													{#each parentChildRelationFields() as relationField}
														{@const childRecords = childRecordsForRelationField(relationField)}
														{@const childFields = childGridFieldsForRelationField(relationField)}
														{@const childForm = childFormForRelationField(relationField)}

														<section
															class="overflow-hidden rounded-card border border-line"
															data-child-table-grid={relationField.key}
															data-child-table-relation-key={childRelationKeyForField(
																relationField
															)}
															data-child-table-child-form={childForm?.key}
														>
															<div
																class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
															>
																<div class="min-w-0">
																	<div
																		class="truncate text-xs font-semibold uppercase text-fg-muted"
																		data-child-table-grid-title={relationField.key}
																	>
																		{fieldLabel(relationField)}
																	</div>

																	<div
																		class="mt-0.5 text-[11px] text-fg-muted"
																		data-child-table-grid-meta={relationField.key}
																	>
																		{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(
																			relationField
																		)}
																	</div>
																</div>

																<span
																	class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
																	data-child-table-grid-count={`${relationField.key}:${childRecords.length}`}
																	>{$t('forms.groupRecordCount', {
																		values: { count: childRecords.length }
																	})}</span
																>
															</div>

															{#if childRecords.length > 0}
																<div class="overflow-x-auto">
																	<table
																		class="min-w-full divide-y divide-line text-xs"
																		data-child-table-grid-table={relationField.key}
																	>
																		<thead class="bg-nav">
																			<tr>
																				<th
																					class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
																					>{$t('forms.recordTitle')}</th
																				>

																				{#each childFields as childField}
																					<th
																						class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
																						data-child-table-grid-column={`${relationField.key}:${childField.key}`}
																						>{fieldLabel(childField)}</th
																					>
																				{/each}

																				<th
																					class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
																					>{$t('common.actions')}</th
																				>
																			</tr>
																		</thead>

																		<tbody class="divide-y divide-line">
																			{#each childRecords as child}
																				<tr
																					data-child-table-grid-row={`${relationField.key}:${child.record.id}`}
																				>
																					<td class="px-3 py-2 font-medium text-fg"
																						>{child.record.title}</td
																					>

																					{#each childFields as childField}
																						<td
																							class="max-w-56 px-3 py-2 text-fg"
																							data-child-table-grid-cell={`${child.record.id}:${childField.key}`}
																							><span class="line-clamp-2 break-words"
																								>{displayValue(
																									child.record.values[childField.key]
																								)}</span
																							></td
																						>
																					{/each}

																					<td class="px-3 py-2 text-right">
																						<div class="flex justify-end gap-1">
																							<button
																								type="button"
																								class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																								onclick={() => startEditChildRecord(child)}
																								aria-label={$t('forms.editChildRecord')}
																								data-child-table-edit-row={`${relationField.key}:${child.record.id}`}
																								><Pencil
																									class="h-4 w-4"
																									aria-hidden="true"
																								/></button
																							>

																							<button
																								type="button"
																								class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																								disabled={s.deletingChildRecordId ===
																									child.record.id}
																								onclick={() => deleteChildRecord(child)}
																								aria-label={$t('forms.deleteChildRecord')}
																								data-child-table-delete-row={`${relationField.key}:${child.record.id}`}
																								><Trash2
																									class="h-4 w-4"
																									aria-hidden="true"
																								/></button
																							>
																						</div>
																					</td>
																				</tr>
																			{/each}
																		</tbody>
																	</table>
																</div>
															{:else}
																<p
																	class="px-3 py-3 text-sm text-fg-muted"
																	data-child-table-grid-empty={relationField.key}
																>
																	{$t('forms.noChildRecords')}
																</p>
															{/if}
														</section>
													{/each}
												</div>
											{:else if s.selectedRecordChildren.length > 0}
												<div class="overflow-hidden rounded-control border border-line">
													<table class="min-w-full divide-y divide-line text-xs">
														<thead class="bg-nav">
															<tr>
																<th class="px-3 py-2 text-left font-medium text-fg-muted"
																	>{$t('forms.recordTitle')}</th
																>

																<th class="px-3 py-2 text-left font-medium text-fg-muted"
																	>{$t('forms.relationKey')}</th
																>

																<th class="px-3 py-2 text-left font-medium text-fg-muted"
																	>{$t('forms.fields')}</th
																>

																<th class="px-3 py-2 text-right font-medium text-fg-muted"
																	>{$t('common.actions')}</th
																>
															</tr>
														</thead>

														<tbody class="divide-y divide-line">
															{#each s.selectedRecordChildren as child}
																<tr>
																	<td class="px-3 py-2 font-medium text-fg">{child.record.title}</td
																	>

																	<td class="px-3 py-2 font-mono text-fg-muted"
																		>{child.relation_key}</td
																	>

																	<td class="px-3 py-2 text-fg">
																		{#each childRecordFields(child).slice(0, 4) as childField, index}
																			{#if index > 0}
																				<span class="mx-1 text-fg-muted">/</span>
																			{/if}

																			<span
																				>{fieldLabel(childField)}: {displayValue(
																					child.record.values[childField.key]
																				)}</span
																			>
																		{/each}
																	</td>

																	<td class="px-3 py-2 text-right">
																		<div class="flex justify-end gap-1">
																			<button
																				type="button"
																				class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																				onclick={() => startEditChildRecord(child)}
																				aria-label={$t('forms.editChildRecord')}
																				><Pencil class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																				disabled={s.deletingChildRecordId === child.record.id}
																				onclick={() => deleteChildRecord(child)}
																				aria-label={$t('forms.deleteChildRecord')}
																				><Trash2 class="h-4 w-4" aria-hidden="true" /></button
																			>
																		</div>
																	</td>
																</tr>
															{/each}
														</tbody>
													</table>
												</div>
											{:else}
												<p class="text-sm text-fg-muted">
													{$t('forms.noChildRecords')}
												</p>
											{/if}
										</div>

										<div class="mb-3 flex items-center gap-2">
											<Link class="h-4 w-4 text-fg-muted" aria-hidden="true" />

											<h4 class="text-sm font-semibold text-fg">
												{$t('forms.recordLinks')}
											</h4>
										</div>

										<div class="space-y-2">
											{#each s.selectedRecordLinks as item}
												<div class="rounded-card border border-line px-3 py-2 text-xs">
													<div class="font-medium text-fg">
														{item.relation_key} / {item.relation_type}
													</div>

													<div class="mt-1 truncate font-mono text-fg-muted">
														{item.target_type}:{item.target_id}
													</div>
												</div>
											{/each}

											{#if s.selectedRecordLinks.length === 0}
												<p class="text-sm text-fg-muted">
													{$t('forms.noLinkedData')}
												</p>
											{/if}
										</div>

										<div class="mt-3 space-y-2">
											<input
												class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
												bind:value={s.linkDraft.target_id}
												placeholder={$t('forms.targetPlaceholder')}
											/>

											<div class="grid grid-cols-2 gap-2">
												<input
													class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
													bind:value={s.linkDraft.relation_key}
													placeholder={$t('forms.relationKeyPlaceholder')}
												/>

												<input
													class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
													bind:value={s.linkDraft.relation_type}
													placeholder={$t('forms.relationTypePlaceholder')}
												/>
											</div>

											<Button
												size="sm"
												variant="secondary"
												loading={s.linkSubmitting}
												disabled={!s.linkDraft.target_id.trim()}
												onclick={handleCreateLink}
											>
												<Link class="mr-2 h-4 w-4" aria-hidden="true" />
												{$t('forms.addLink')}
											</Button>
										</div>

										<div
											class="mt-4 border-t border-line pt-4"
											data-record-detail-comments={s.selectedRecord.id}
											data-record-detail-comment-count={s.selectedRecordComments.length}
										>
											<div class="mb-3 flex items-center gap-2">
												<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

												<h4 class="text-sm font-semibold text-fg">
													{$t('forms.recordComments')}
												</h4>
											</div>

											<div class="space-y-2">
												{#each s.selectedRecordComments.slice(0, 5) as comment}
													<div
														class="rounded-card border border-line px-3 py-2 text-xs"
														data-record-detail-comment={comment.id}
													>
														<div class="flex flex-wrap items-center justify-between gap-2">
															<span
																class="font-medium text-fg"
																data-record-detail-comment-author={comment.id}
																>{comment.author_name ||
																	comment.author_email ||
																	comment.author_id?.slice(0, 8) ||
																	'system'}</span
															>

															<span
																class="font-mono text-[11px] text-fg-muted"
																data-record-detail-comment-created={comment.id}
																>{formatDate(comment.created_at)}</span
															>
														</div>

														<p
															class="mt-2 whitespace-pre-wrap text-sm leading-relaxed text-fg"
															data-record-detail-comment-body={comment.id}
														>
															{comment.body}
														</p>
													</div>
												{/each}

												{#if s.recordCommentsLoading && s.selectedRecordComments.length === 0}
													<p class="text-sm text-fg-muted">
														{$t('common.loading')}
													</p>
												{:else if s.selectedRecordComments.length === 0}
													<p
														class="text-sm text-fg-muted"
														data-record-detail-comments-empty={s.selectedRecord.id}
													>
														{$t('forms.noRecordComments')}
													</p>
												{/if}

												<div class="space-y-2 pt-1">
													<textarea
														class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
														bind:value={s.recordCommentDraft}
														placeholder={$t('forms.recordCommentPlaceholder')}
														data-record-detail-comment-input={s.selectedRecord.id}></textarea>

													<span data-record-detail-comment-submit={s.selectedRecord.id}>
														<Button
															size="sm"
															variant="secondary"
															loading={s.recordCommentSubmitting}
															disabled={!s.recordCommentDraft.trim()}
															onclick={handleCreateRecordComment}>{$t('forms.addComment')}</Button
														>
													</span>
												</div>
											</div>
										</div>

										{#if selectedRecordAttachmentFields().length > 0}
											<div
												class="mt-4 border-t border-line pt-4"
												data-record-detail-attachments={s.selectedRecord.id}
												data-record-detail-attachment-count={s.recordAttachments.filter(
													(attachment) => !attachment.archived_at
												).length}
											>
												<div class="mb-3 flex items-center gap-2">
													<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

													<h4 class="text-sm font-semibold text-fg">
														{$t('forms.attachmentConfig')}
													</h4>
												</div>

												<div class="space-y-3">
													{#each selectedRecordAttachmentFields() as field}
														{@const metadataAttachments = fieldAttachments(field.key)}
														{@const valueUrls = selectedRecordAttachmentValueUrls(field)}

														<section
															class="rounded-card border border-line px-3 py-2 text-xs"
															data-record-detail-attachment-field={field.key}
															data-record-detail-attachment-field-count={`${field.key}:${metadataAttachments.length + valueUrls.length}`}
														>
															<div class="mb-2 font-medium text-fg">
																{fieldLabel(field)}
															</div>

															<div class="space-y-2">
																{#each metadataAttachments as attachment}
																	<div
																		class="flex flex-wrap items-center gap-2 rounded bg-nav px-2 py-1.5"
																		data-record-detail-attachment-item={attachment.id}
																	>
																		{#if attachmentIsPreviewableImage(attachment)}
																			<img
																				src={attachmentThumbnailUrl(attachment)}
																				alt={attachment.file_name}
																				class="h-10 w-10 rounded object-cover ring-1 ring-line"
																				data-record-detail-attachment-thumbnail={attachment.id}
																			/>
																		{/if}

																		<div class="min-w-0 flex-1">
																			<div class="truncate font-medium text-fg">
																				{attachment.file_name}
																			</div>

																			<div
																				class="truncate font-mono text-[11px] text-fg-muted"
																				data-record-detail-attachment-url={attachment.id}
																			>
																				{attachment.url}
																			</div>
																		</div>

																		<a
																			href={attachmentSignedUrl(attachment) || undefined}
																			target="_blank"
																			rel="noreferrer"
																			class="rounded px-2 py-1 font-medium text-fg underline hover:bg-surface"
																			data-record-detail-attachment-open={attachment.id}
																			data-record-detail-attachment-signed-url={attachmentSignedUrl(
																				attachment
																			)}>{$t('forms.attachmentOpen')}</a
																		>

																		<a
																			href={attachmentSignedUrl(attachment) || undefined}
																			download={attachment.file_name}
																			class="rounded px-2 py-1 font-medium text-fg hover:bg-surface"
																			data-record-detail-attachment-download={attachment.id}
																			data-record-detail-attachment-signed-download={attachmentSignedUrl(
																				attachment
																			)}>{$t('forms.attachmentDownload')}</a
																		>
																	</div>
																{/each}

																{#each valueUrls as url}
																	<div
																		class="flex flex-wrap items-center gap-2 rounded bg-nav px-2 py-1.5"
																		data-record-detail-attachment-value={`${field.key}:${url}`}
																	>
																		<div
																			class="min-w-0 flex-1 truncate font-mono text-[11px] text-fg-muted"
																		>
																			{url}
																		</div>

																		{#if safeLinkUrl(url)}
																			<a
																				href={safeLinkUrl(url)}
																				target="_blank"
																				rel="noreferrer"
																				class="rounded px-2 py-1 font-medium text-fg underline hover:bg-surface"
																				data-record-detail-attachment-value-open={field.key}
																				>{$t('forms.attachmentOpen')}</a
																			>
																		{:else}
																			<span
																				class="rounded px-2 py-1 text-[11px] font-medium text-warning"
																				data-record-detail-attachment-value-blocked={field.key}
																				>{$t('forms.attachmentUrlUnsafe')}</span
																			>
																		{/if}
																	</div>
																{/each}

																{#if metadataAttachments.length === 0 && valueUrls.length === 0}
																	<p
																		class="text-sm text-fg-muted"
																		data-record-detail-attachment-empty={field.key}
																	>
																		{$t('forms.noRecordAttachments')}
																	</p>
																{/if}
															</div>
														</section>
													{/each}
												</div>
											</div>
										{/if}

										<div
											class="mt-4 border-t border-line pt-4"
											data-record-detail-event-history={s.selectedRecord.id}
											data-record-detail-event-count={s.selectedRecordEvents.length}
										>
											<div class="mb-3 flex items-center gap-2">
												<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

												<h4 class="text-sm font-semibold text-fg">
													{$t('forms.timelineSourceEvents')}
												</h4>
											</div>

											<div class="space-y-2">
												{#each s.selectedRecordEvents.slice(0, 5) as event}
													<details
														class="rounded-card border border-line px-3 py-2 text-xs"
														data-record-detail-event={event.id}
														data-record-detail-event-type={event.event_type}
													>
														<summary class="cursor-pointer list-none">
															<div class="flex flex-wrap items-start justify-between gap-2">
																<div class="min-w-0">
																	<div
																		class="truncate font-medium text-fg"
																		data-record-detail-event-title={event.id}
																	>
																		{timelineEventTitle(event)}
																	</div>

																	<div
																		class="mt-1 font-mono text-[11px] text-fg-muted"
																		data-record-detail-event-aggregate={event.id}
																	>
																		{timelineEventAggregateKey(event)}
																	</div>
																</div>

																<span
																	class="rounded bg-surface px-2 py-0.5 font-mono text-[11px] text-fg-muted"
																	data-record-detail-event-created={event.id}
																	>{formatDate(event.created_at)}</span
																>
															</div>

															<div
																class="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-fg-muted"
															>
																<span data-record-detail-event-kind={event.id}
																	>{event.event_type}</span
																>

																{#if event.actor_id}
																	<span data-record-detail-event-actor={event.id}
																		>{$t('forms.eventActor')} {event.actor_id}</span
																	>
																{/if}
															</div>
														</summary>

														<div class="mt-3 grid gap-2 lg:grid-cols-2">
															<pre
																class="max-h-40 overflow-auto rounded bg-nav p-2 font-mono text-[11px] text-fg-muted"
																data-record-detail-event-payload={event.id}>{timelineEventJson(
																	event.payload
																)}</pre>

															<pre
																class="max-h-40 overflow-auto rounded bg-nav p-2 font-mono text-[11px] text-fg-muted"
																data-record-detail-event-source={event.id}>{timelineEventJson(
																	event.source
																)}</pre>
														</div>
													</details>
												{/each}

												{#if s.selectedRecordEvents.length === 0}
													<p
														class="text-sm text-fg-muted"
														data-record-detail-event-empty={s.selectedRecord.id}
													>
														{$t('forms.noRecordEvents')}
													</p>
												{/if}
											</div>
										</div>
									</div>
								{:else}
									<div class="p-6 text-sm text-fg-muted">
										{$t('forms.selectRecordPrompt')}
									</div>
								{/if}
							</aside>
						{/if}
					</div>
				{/if}
			</section>
		{/if}
	{/if}
</div>

<ImportWizardModal />

<DesignerConfirmModal />
