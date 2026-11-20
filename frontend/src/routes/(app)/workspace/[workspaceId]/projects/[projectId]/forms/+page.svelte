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

	import RecordDetailAside from '#lib/components/forms/page/RecordDetailAside.svelte';

	// forms-split slot W2-01
	import LayoutSettingsContent from '#lib/components/forms/page/LayoutSettingsContent.svelte';

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
									<LayoutSettingsContent />
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
							<RecordDetailAside />
						{/if}
					</div>
				{/if}
			</section>
		{/if}
	{/if}
</div>

<ImportWizardModal />

<DesignerConfirmModal />
