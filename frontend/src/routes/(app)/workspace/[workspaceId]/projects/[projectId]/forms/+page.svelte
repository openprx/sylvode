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

	import FormWorkspace from '#lib/components/forms/page/FormWorkspace.svelte';

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
			<FormWorkspace />
		{/if}
	{/if}
</div>

<ImportWizardModal />

<DesignerConfirmModal />
