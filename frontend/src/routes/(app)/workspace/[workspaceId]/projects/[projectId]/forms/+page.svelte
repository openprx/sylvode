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

	// forms-split slot W1-05

	// forms-split slot W1-06

	// forms-split slot W1-07

	// forms-split slot W1-08

	// forms-split slot W1-09

	// forms-split slot W1-10

	// forms-split slot W1-11

	// forms-split slot W1-12

	// forms-split slot W1-13

	// forms-split slot W1-14

	// forms-split slot W1-15

	// forms-split slot W1-16

	// forms-split slot W1-17

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
					<section
						class="rounded-card border border-line bg-raised p-4"
						data-form-mode="import-export"
					>
						<div class="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
							<div>
								<div class="flex items-center gap-2">
									<FileUp class="h-4 w-4 text-fg-muted" aria-hidden="true" />

									<h3 class="text-base font-semibold text-fg">
										{$t('forms.modes.importExport')}
									</h3>
								</div>
							</div>

							<Button size="sm" onclick={openImportRecords}>
								<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
								{$t('forms.importRecords')}
							</Button>
						</div>

						<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface"
								onclick={exportImportTemplate}
								disabled={!s.selectedForm || s.visibleFields.length === 0}
								data-import-template-export
								data-import-template-exported={s.exportedImportTemplate ? 'true' : undefined}
							>
								<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.downloadImportTemplate')}</span>
							</button>

							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
								onclick={exportCurrentView}
								disabled={s.exportingRecords}
							>
								<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.exportCurrentView')}</span>
							</button>

							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
								onclick={exportCurrentViewJson}
								disabled={s.exportingRecords}
								data-current-view-export-json
								data-current-view-json-record-count={s.exportedCurrentViewJsonRecordCount ?? ''}
							>
								<FileText class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.exportCurrentViewJson')}</span>
							</button>

							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
								onclick={exportAttachmentManifest}
								disabled={s.exportingRecords}
								data-attachment-manifest-export
								data-attachment-manifest-count={s.exportedAttachmentManifestCount ?? ''}
							>
								<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.exportAttachmentManifest')}</span>
							</button>

							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
								onclick={() => void exportAttachmentBundle()}
								disabled={s.exportingRecords}
								data-attachment-bundle-export
								data-attachment-bundle-count={s.exportedAttachmentBundleCount ?? ''}
							>
								<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.exportAttachmentBundle')}</span>
							</button>

							<button
								type="button"
								class="flex min-h-24 flex-col items-start justify-between rounded-card border border-line bg-nav p-3 text-left text-sm hover:border-input hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
								onclick={() => void exportAttachmentPackage()}
								disabled={s.exportingRecords}
								data-attachment-package-export
								data-attachment-package-count={s.exportedAttachmentPackageCount ?? ''}
								data-attachment-package-file-count={s.exportedAttachmentPackageFileCount ?? ''}
							>
								<FileDown class="h-4 w-4 text-fg-muted" aria-hidden="true" />

								<span class="font-medium text-fg">{$t('forms.exportAttachmentPackage')}</span>
							</button>
						</div>
					</section>
				{:else if s.activeMode === 'design' || s.activeMode === 'layout'}
					<div
						class="grid min-w-0 gap-4"
						data-form-mode={s.activeMode === 'design' ? 'field-design' : 'detail-layout'}
					>
						{#if s.activeMode === 'design'}
							<FormDesignerWorkspace
								form={s.selectedForm}
								fieldDrafts={s.designerFieldDrafts}
								{fieldTypeOptions}
								selectedIndex={s.selectedDesignerFieldIndex}
								selectedFieldUsage={s.selectedDesignerFieldUsage}
								selectedFieldDependencies={s.selectedDesignerFieldDependencies}
								hiddenFieldKeys={s.memberHiddenFieldKeys}
								lockedFieldKeys={s.memberLockedFieldKeys}
								usageLoading={s.designerUsageLoading}
								saving={s.designerSaving}
								onSelectField={(index) => (s.selectedDesignerFieldIndex = index)}
								onAppendField={addDesignerField}
								onUpdateField={updateDesignerField}
								onDuplicateField={duplicateDesignerField}
								onRemoveField={removeDesignerField}
								onMoveField={moveDesignerField}
								onSave={saveDesignerSchema}
							/>
						{/if}

						{#if s.activeMode === 'layout'}
							<div class="space-y-4">
								<div
									class="flex flex-wrap gap-1 rounded-card border border-line bg-raised p-1"
									data-detail-layout-tabs
								>
									{#each detailLayoutModes as mode}
										<button
											type="button"
											class="inline-flex h-9 items-center rounded-control px-3 text-sm font-medium transition-colors {s.detailLayoutMode ===
											mode.key
												? 'rf-filled bg-primary text-on-primary'
												: 'text-fg-muted hover:bg-surface hover:text-fg'}"
											aria-current={s.detailLayoutMode === mode.key ? 'page' : undefined}
											data-detail-layout-tab={mode.key}
											onclick={() => (s.detailLayoutMode = mode.key)}>{$t(mode.labelKey)}</button
										>
									{/each}
								</div>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'header'
										? ''
										: 'hidden'}"
									data-detail-header-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3">
										<h3 class="text-base font-semibold text-fg">
											{$t('forms.detailHeader')}
										</h3>

										<p class="mt-1 text-sm text-fg-muted">
											{$t('forms.detailHeaderDescription')}
										</p>
									</div>

									<div class="grid gap-3">
										<label class="space-y-1">
											<span class="text-xs font-medium uppercase text-fg-muted"
												>{$t('forms.detailHeaderSubtitleField')}</span
											>

											<select
												value={s.detailHeaderDraft.subtitleField}
												disabled={s.detailHeaderSaving}
												class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
												data-detail-header-subtitle-field
												onchange={(event) =>
													setDetailHeaderDraft({
														subtitleField:
															event.currentTarget instanceof HTMLSelectElement
																? event.currentTarget.value
																: ''
													})}
											>
												<option value="">{$t('forms.detailHeaderNoField')}</option>

												{#each s.detailVisibleFields as field}
													<option value={field.key}>{fieldLabel(field)}</option>
												{/each}
											</select>
										</label>

										<label class="space-y-1">
											<span class="text-xs font-medium uppercase text-fg-muted"
												>{$t('forms.detailHeaderBadgeField')}</span
											>

											<select
												value={s.detailHeaderDraft.badgeField}
												disabled={s.detailHeaderSaving}
												class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
												data-detail-header-badge-field
												onchange={(event) =>
													setDetailHeaderDraft({
														badgeField:
															event.currentTarget instanceof HTMLSelectElement
																? event.currentTarget.value
																: ''
													})}
											>
												<option value="">{$t('forms.detailHeaderNoField')}</option>

												{#each s.detailVisibleFields as field}
													<option value={field.key}>{fieldLabel(field)}</option>
												{/each}
											</select>
										</label>
									</div>

									<div class="mt-3 flex justify-end">
										<span data-detail-header-save>
											<Button size="sm" loading={s.detailHeaderSaving} onclick={saveDetailHeader}
												>{$t('forms.saveDetailHeader')}</Button
											>
										</span>
									</div>
								</section>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'highlights'
										? ''
										: 'hidden'}"
									data-detail-highlights-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3 flex items-start justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-fg">
												{$t('forms.detailHighlights')}
											</h3>

											<p class="mt-1 text-sm text-fg-muted">
												{$t('forms.detailHighlightsDescription')}
											</p>
										</div>

										{#if s.detailHighlightSaving}
											<span class="text-xs font-medium text-fg-muted">{$t('common.saving')}</span>
										{/if}
									</div>

									<div class="space-y-2">
										{#each s.detailVisibleFields as field}
											{@const highlightIndex = s.detailHighlightDraft.fields.indexOf(field.key)}
											{@const isChecked = highlightIndex >= 0}

											<div
												class="grid min-h-14 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-card border border-line px-3 py-2"
												data-detail-highlight-field={field.key}
												data-detail-highlight-selected={`${field.key}:${isChecked}`}
											>
												<label class="flex h-10 w-10 items-center justify-center">
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-input accent-primary"
														checked={isChecked}
														disabled={s.detailHighlightSaving ||
															(!isChecked && s.detailHighlightDraft.fields.length >= 4)}
														aria-label={fieldLabel(field)}
														data-detail-highlight-toggle={field.key}
														onchange={(event) =>
															setDetailHighlightField(
																field.key,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
												</label>

												<div class="min-w-0">
													<div class="truncate text-sm font-medium text-fg">
														{fieldLabel(field)}
													</div>

													<p class="mt-0.5 truncate font-mono text-xs text-fg-muted">
														{field.key}
													</p>
												</div>

												<div class="flex items-center gap-1">
													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={!isChecked || highlightIndex === 0 || s.detailHighlightSaving}
														aria-label={$t('forms.moveHighlightUp')}
														title={$t('forms.moveHighlightUp')}
														data-detail-highlight-move-up={field.key}
														onclick={() => moveDetailHighlightField(field.key, -1)}
														><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
													>

													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={!isChecked ||
															highlightIndex === s.detailHighlightDraft.fields.length - 1 ||
															s.detailHighlightSaving}
														aria-label={$t('forms.moveHighlightDown')}
														title={$t('forms.moveHighlightDown')}
														data-detail-highlight-move-down={field.key}
														onclick={() => moveDetailHighlightField(field.key, 1)}
														><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
													>
												</div>
											</div>
										{/each}
									</div>

									<div class="mt-3 flex items-center justify-between gap-3">
										<span class="text-xs text-fg-muted"
											>{$t('forms.detailHighlightsLimit', {
												values: { count: s.detailHighlightDraft.fields.length }
											})}</span
										>

										<span data-detail-highlights-save>
											<Button
												size="sm"
												loading={s.detailHighlightSaving}
												onclick={saveDetailHighlights}>{$t('forms.saveDetailHighlights')}</Button
											>
										</span>
									</div>
								</section>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'composition'
										? ''
										: 'hidden'}"
									data-detail-page-composition-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3">
										<h3 class="text-base font-semibold text-fg">
											{$t('forms.detailPageComposition')}
										</h3>

										<p class="mt-1 text-sm text-fg-muted">
											{$t('forms.detailPageCompositionDescription')}
										</p>
									</div>

									<label class="space-y-1">
										<span class="text-xs font-medium uppercase text-fg-muted"
											>{$t('forms.detailPageWidgetRegion')}</span
										>

										<select
											value={s.detailPageCompositionDraft.widgetRegion}
											disabled={s.detailPageCompositionSaving}
											class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
											data-detail-page-widget-region
											onchange={(event) =>
												(s.detailPageCompositionDraft = {
													...s.detailPageCompositionDraft,
													widgetRegion:
														event.currentTarget instanceof HTMLSelectElement
															? detailPageWidgetRegion(event.currentTarget.value)
															: 'sidebar'
												})}
										>
											<option value="sidebar">{$t('forms.detailPageWidgetRegionSidebar')}</option>
											<option value="below">{$t('forms.detailPageWidgetRegionBelow')}</option>
										</select>
									</label>

									<label class="mt-3 block space-y-1">
										<span class="text-xs font-medium uppercase text-fg-muted"
											>{$t('forms.detailPageFieldColumns')}</span
										>

										<select
											value={String(s.detailPageCompositionDraft.fieldColumns)}
											disabled={s.detailPageCompositionSaving}
											class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
											data-detail-page-field-columns
											onchange={(event) =>
												(s.detailPageCompositionDraft = {
													...s.detailPageCompositionDraft,
													fieldColumns:
														event.currentTarget instanceof HTMLSelectElement
															? detailPageFieldColumns(event.currentTarget.value)
															: 2
												})}
										>
											<option value="1">{$t('forms.detailPageFieldColumnsOne')}</option>
											<option value="2">{$t('forms.detailPageFieldColumnsTwo')}</option>
										</select>
									</label>

									<div class="mt-3 flex justify-end">
										<span data-detail-page-composition-save>
											<Button
												size="sm"
												loading={s.detailPageCompositionSaving}
												onclick={saveDetailPageComposition}
												>{$t('forms.saveDetailPageComposition')}</Button
											>
										</span>
									</div>
								</section>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'sections'
										? ''
										: 'hidden'}"
									data-detail-layout-section-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3 flex items-start justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-fg">
												{$t('forms.detailLayoutSections')}
											</h3>

											<p class="mt-1 text-sm text-fg-muted">
												{$t('forms.detailLayoutSectionsDescription')}
											</p>
										</div>

										<Button size="sm" variant="secondary" onclick={addDetailLayoutSection}>
											<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
											{$t('forms.addDetailSection')}
										</Button>
									</div>

									<div class="space-y-3">
										{#each s.detailLayoutSectionDrafts as section, sectionIndex}
											<div
												class="rounded-card border border-line p-3"
												data-detail-layout-section-row={section.key}
											>
												<div
													class="mb-3 grid grid-cols-[minmax(0,1fr)_auto] items-end gap-2 md:grid-cols-[minmax(0,1fr)_120px_132px_auto]"
												>
													<label class="min-w-0 space-y-1">
														<span class="text-xs font-medium uppercase text-fg-muted"
															>{$t('forms.detailLayoutSectionTitle')}</span
														>

														<input
															value={section.title}
															class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
															data-detail-layout-section-title={section.key}
															oninput={(event) =>
																updateDetailLayoutSectionTitle(
																	sectionIndex,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
														/>
													</label>

													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-fg-muted"
															>{$t('forms.detailLayoutSectionColumns')}</span
														>

														<select
															value={String(section.columns)}
															disabled={s.detailLayoutSectionSaving}
															class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
															data-detail-layout-section-columns={section.key}
															onchange={(event) =>
																updateDetailLayoutSectionColumns(
																	sectionIndex,
																	event.currentTarget instanceof HTMLSelectElement
																		? detailLayoutSectionColumns(event.currentTarget.value)
																		: 2
																)}
														>
															<option value="1">{$t('forms.detailLayoutSectionColumnsOne')}</option>
															<option value="2">{$t('forms.detailLayoutSectionColumnsTwo')}</option>
															<option value="3"
																>{$t('forms.detailLayoutSectionColumnsThree')}</option
															>
														</select>
													</label>

													<label class="space-y-1">
														<span class="text-xs font-medium uppercase text-fg-muted"
															>{$t('forms.detailLayoutSectionDensity')}</span
														>

														<select
															value={section.density}
															disabled={s.detailLayoutSectionSaving}
															class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
															data-detail-layout-section-density={section.key}
															onchange={(event) =>
																updateDetailLayoutSectionDensity(
																	sectionIndex,
																	event.currentTarget instanceof HTMLSelectElement &&
																		event.currentTarget.value === 'compact'
																		? 'compact'
																		: 'comfortable'
																)}
														>
															<option value="comfortable"
																>{$t('forms.detailLayoutSectionDensityComfortable')}</option
															>
															<option value="compact"
																>{$t('forms.detailLayoutSectionDensityCompact')}</option
															>
														</select>
													</label>

													<div class="flex items-center gap-1">
														<button
															type="button"
															class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
															disabled={sectionIndex === 0 || s.detailLayoutSectionSaving}
															aria-label={$t('forms.moveSectionUp')}
															title={$t('forms.moveSectionUp')}
															data-detail-layout-section-move-up={section.key}
															onclick={() => moveDetailLayoutSection(sectionIndex, -1)}
															><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
														>

														<button
															type="button"
															class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
															disabled={sectionIndex === s.detailLayoutSectionDrafts.length - 1 ||
																s.detailLayoutSectionSaving}
															aria-label={$t('forms.moveSectionDown')}
															title={$t('forms.moveSectionDown')}
															data-detail-layout-section-move-down={section.key}
															onclick={() => moveDetailLayoutSection(sectionIndex, 1)}
															><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
														>

														<button
															type="button"
															class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
															disabled={s.detailLayoutSectionDrafts.length <= 1 ||
																s.detailLayoutSectionSaving}
															aria-label={$t('forms.removeDetailSection')}
															title={$t('forms.removeDetailSection')}
															data-detail-layout-section-remove={section.key}
															onclick={() => removeDetailLayoutSection(sectionIndex)}
															><Trash2 class="h-4 w-4" aria-hidden="true" /></button
														>
													</div>
												</div>

												<label class="mb-3 block space-y-1">
													<span class="text-xs font-medium uppercase text-fg-muted"
														>{$t('forms.detailLayoutSectionDescription')}</span
													>

													<textarea
														value={section.description}
														rows="2"
														disabled={s.detailLayoutSectionSaving}
														placeholder={$t('forms.detailLayoutSectionDescriptionPlaceholder')}
														class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
														data-detail-layout-section-description={section.key}
														oninput={(event) =>
															updateDetailLayoutSectionDescription(
																sectionIndex,
																event.currentTarget instanceof HTMLTextAreaElement
																	? event.currentTarget.value
																	: ''
															)}></textarea>
												</label>

												<label class="mb-3 flex items-center gap-2 text-sm text-fg">
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-input accent-primary"
														checked={section.hideEmptyFields}
														disabled={s.detailLayoutSectionSaving}
														data-detail-layout-section-hide-empty-fields={section.key}
														onchange={(event) =>
															updateDetailLayoutSectionHideEmptyFields(
																sectionIndex,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>

													<span>{$t('forms.detailLayoutSectionHideEmptyFields')}</span>
												</label>

												<div class="mb-3 grid gap-2 sm:grid-cols-2">
													<label class="flex items-center gap-2 text-sm text-fg">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={section.collapsible}
															disabled={s.detailLayoutSectionSaving}
															data-detail-layout-section-collapsible={section.key}
															onchange={(event) =>
																updateDetailLayoutSectionCollapsible(
																	sectionIndex,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailLayoutSectionCollapsible')}</span>
													</label>

													<label class="flex items-center gap-2 text-sm text-fg">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={section.defaultCollapsed}
															disabled={s.detailLayoutSectionSaving || !section.collapsible}
															data-detail-layout-section-default-collapsed={section.key}
															onchange={(event) =>
																updateDetailLayoutSectionDefaultCollapsed(
																	sectionIndex,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailLayoutSectionDefaultCollapsed')}</span>
													</label>
												</div>

												<div class="space-y-1.5">
													{#each s.detailVisibleFields as field}
														{@const assignedSectionKey = detailLayoutFieldAssignedTo(field.key)}
														{@const fieldIndex = section.fields.indexOf(field.key)}

														<div
															class="grid min-h-9 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-control px-2 py-1 text-sm hover:bg-surface"
															data-detail-layout-section-field={`${section.key}:${field.key}`}
															data-detail-layout-section-field-assigned={assignedSectionKey ||
																'unassigned'}
														>
															<input
																type="checkbox"
																class="h-4 w-4 rounded border-input accent-primary"
																checked={section.fields.includes(field.key)}
																disabled={s.detailLayoutSectionSaving}
																aria-label={`${$t('forms.detailLayoutField')}: ${fieldLabel(field)} / ${section.title}`}
																onchange={(event) =>
																	setDetailLayoutSectionField(
																		sectionIndex,
																		field.key,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)}
															/>

															<div class="min-w-0">
																<span class="block truncate text-fg">{fieldLabel(field)}</span>

																{#if assignedSectionKey && assignedSectionKey !== section.key}
																	<span class="text-xs text-fg-muted"
																		>{$t('forms.detailLayoutFieldAssigned')}</span
																	>
																{/if}
															</div>

															<div class="flex items-center gap-1">
																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	disabled={fieldIndex <= 0 || s.detailLayoutSectionSaving}
																	aria-label={$t('forms.moveFieldUp')}
																	title={$t('forms.moveFieldUp')}
																	data-detail-layout-field-move-up={`${section.key}:${field.key}`}
																	onclick={() =>
																		moveDetailLayoutSectionField(sectionIndex, field.key, -1)}
																	><ArrowUp class="h-3.5 w-3.5" aria-hidden="true" /></button
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	disabled={fieldIndex < 0 ||
																		fieldIndex === section.fields.length - 1 ||
																		s.detailLayoutSectionSaving}
																	aria-label={$t('forms.moveFieldDown')}
																	title={$t('forms.moveFieldDown')}
																	data-detail-layout-field-move-down={`${section.key}:${field.key}`}
																	onclick={() =>
																		moveDetailLayoutSectionField(sectionIndex, field.key, 1)}
																	><ArrowDown class="h-3.5 w-3.5" aria-hidden="true" /></button
																>
															</div>
														</div>
													{/each}
												</div>
											</div>
										{/each}
									</div>

									<div class="mt-3 flex justify-end">
										<span data-detail-layout-section-save>
											<Button
												size="sm"
												loading={s.detailLayoutSectionSaving}
												onclick={saveDetailLayoutSections}>{$t('forms.saveDetailSections')}</Button
											>
										</span>
									</div>
								</section>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'widgets'
										? ''
										: 'hidden'}"
									data-detail-page-widget-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3 flex items-start justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-fg">
												{$t('forms.detailPageWidgets')}
											</h3>

											<p class="mt-1 text-sm text-fg-muted">
												{$t('forms.detailPageWidgetsDescription')}
											</p>
										</div>

										{#if s.detailPageWidgetSaving}
											<span class="text-xs font-medium text-fg-muted">{$t('common.saving')}</span>
										{/if}
									</div>

									<div class="space-y-2">
										{#each s.selectedDetailPageWidgetEntries as widget, index}
											{@const option = detailPageWidgetOption(widget.key)}

											<div
												class="grid min-h-16 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-card border border-line px-3 py-2"
												data-detail-page-widget-row={widget.key}
												data-detail-page-widget-enabled={`${widget.key}:${widget.enabled}`}
											>
												<label class="flex h-10 w-10 items-center justify-center">
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-input accent-primary"
														checked={widget.enabled}
														disabled={s.detailPageWidgetSaving}
														aria-label={$t(option.labelKey)}
														data-detail-page-widget-toggle={widget.key}
														onchange={(event) =>
															void setDetailPageWidgetEnabled(
																widget.key,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
												</label>

												<div class="min-w-0">
													<div class="truncate text-sm font-medium text-fg">
														{$t(option.labelKey)}
													</div>

													<p class="mt-0.5 text-xs leading-5 text-fg-muted">
														{$t(option.descriptionKey)}
													</p>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetCustomTitle')}</span
														>

														<input
															value={widget.title}
															disabled={s.detailPageWidgetSaving}
															placeholder={$t(option.labelKey)}
															class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-title={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetTitle(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
														/>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetDescription')}</span
														>

														<textarea
															value={widget.description}
															disabled={s.detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetDescriptionPlaceholder')}
															class="w-full resize-y rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-description={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDescription(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}></textarea>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetLimit')}</span
														>

														<input
															type="number"
															min="1"
															max="50"
															value={widget.limit}
															disabled={s.detailPageWidgetSaving}
															placeholder={$t('forms.detailPageWidgetLimitDefault')}
															class="w-28 rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-limit={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetLimit(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
														/>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetDensity')}</span
														>

														<select
															value={widget.density}
															disabled={s.detailPageWidgetSaving}
															class="w-40 rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-density={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDensity(
																	widget.key,
																	event.currentTarget instanceof HTMLSelectElement &&
																		event.currentTarget.value === 'compact'
																		? 'compact'
																		: 'comfortable'
																)}
														>
															<option value="comfortable"
																>{$t('forms.detailPageWidgetDensityComfortable')}</option
															>
															<option value="compact"
																>{$t('forms.detailPageWidgetDensityCompact')}</option
															>
														</select>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.showCount}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-show-count={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetShowCount(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetShowCount')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.hideEmpty}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-hide-empty={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetHideEmpty(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetHideEmpty')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.collapsible}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-collapsible={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetCollapsible(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetCollapsible')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.defaultCollapsed}
															disabled={s.detailPageWidgetSaving || !widget.collapsible}
															data-detail-page-widget-default-collapsed={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDefaultCollapsed(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetDefaultCollapsed')}</span>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetEmptyText')}</span
														>

														<textarea
															value={widget.emptyText}
															disabled={s.detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetEmptyTextPlaceholder')}
															class="w-full resize-y rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-empty-text={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetEmptyText(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}></textarea>
													</label>
												</div>

												<div class="flex items-center gap-1">
													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={index === 0 || s.detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetUp')}
														title={$t('forms.moveWidgetUp')}
														data-detail-page-widget-move-up={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, -1)}
														><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
													>

													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={index === s.selectedDetailPageWidgetEntries.length - 1 ||
															s.detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetDown')}
														title={$t('forms.moveWidgetDown')}
														data-detail-page-widget-move-down={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, 1)}
														><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
													>
												</div>
											</div>
										{/each}
									</div>
								</section>
							</div>
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

														<div class="space-y-2">
															<div class="flex items-center justify-between gap-3">
																<h5 class="text-xs font-semibold uppercase text-fg-muted">
																	{$t('forms.filterConditions')}
																</h5>

																<div class="flex flex-wrap gap-2">
																	<Button
																		size="sm"
																		variant="secondary"
																		onclick={() => addViewDraftFilter(0)}
																		disabled={hasReachedFilterLimit()}
																	>
																		<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																		{$t('forms.addFilter')}
																	</Button>

																	<Button
																		size="sm"
																		variant="secondary"
																		onclick={addViewDraftFilterGroup}
																		disabled={s.viewDraft.filterGroups.length >= 5}
																	>
																		<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																		{$t('forms.addFilterGroup')}
																	</Button>
																</div>
															</div>

															{#if visibleViewDraftFilterGroups().every((group) => group.filters.length === 0)}
																<p
																	class="rounded-card border border-dashed border-input bg-raised px-3 py-2 text-xs text-fg-muted"
																>
																	{$t('forms.noFilterConditions')}
																</p>
															{/if}

															{#if visibleViewDraftFilterGroups().some((group) => group.filters.length > 0)}
																{@const expressionMatchingRecords =
																	viewDraftFilterExpressionRecords()}
																{@const expressionMatchCount =
																	expressionMatchingRecords?.length ?? null}
																{@const expressionRecordIdsText =
																	viewDraftFilterExpressionRecordIdsText(expressionMatchingRecords)}
																{@const expressionJsonText = viewDraftFilterExpressionJsonText()}
																{@const expressionSummaryText =
																	viewDraftFilterExpressionSummaryText()}

																<div class="flex flex-wrap items-end gap-2">
																	<label class="block max-w-xs flex-1 space-y-1">
																		<span class="text-xs font-medium uppercase text-fg-muted"
																			>{$t('forms.filterGroupLogic')}</span
																		>

																		<select
																			value={s.viewDraft.filterRootLogic}
																			onchange={(event) =>
																				updateViewDraftFilterRootLogic(
																					selectValue(event) as ViewFilterLogic
																				)}
																			class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																		>
																			<option value="all">{$t('forms.filterGroupsMatchAll')}</option
																			>
																			<option value="any">{$t('forms.filterGroupsMatchAny')}</option
																			>
																		</select>
																	</label>

																	<span
																		class="rounded-control border border-line bg-surface px-2.5 py-2 text-xs font-semibold text-fg-muted"
																		data-filter-expression-match-count
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		{expressionMatchCount === null
																			? $t('forms.filterMatchCountPending')
																			: $t('forms.filterExpressionMatchCount', {
																					values: { count: expressionMatchCount }
																				})}
																	</span>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			copyViewDraftFilterExpressionRecordIds(
																				expressionMatchingRecords
																			)}
																		disabled={!expressionRecordIdsText}
																		aria-label={$t('forms.copyFilterExpressionRecordIds')}
																		data-filter-expression-copy-ids
																		data-filter-expression-copied={s.copiedFilterExpressionRecordIds ===
																		expressionRecordIdsText
																			? 'true'
																			: undefined}
																	>
																		<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.copyFilterExpressionRecordIds')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			copyViewDraftFilterExpressionJson(expressionJsonText)}
																		disabled={!expressionJsonText}
																		aria-label={$t('forms.copyFilterExpressionJson')}
																		data-filter-expression-copy-json
																		data-filter-expression-json-copied={s.copiedFilterExpressionJson ===
																		expressionJsonText
																			? 'true'
																			: undefined}
																	>
																		<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.copyFilterExpressionJson')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			exportViewDraftFilterExpressionConfigJson(expressionJsonText)}
																		disabled={!expressionJsonText}
																		aria-label={$t('forms.exportFilterExpressionJson')}
																		data-filter-expression-export-config-json
																		data-filter-expression-config-json-exported={s.exportedFilterExpressionJson ===
																		expressionJsonText
																			? 'true'
																			: undefined}
																	>
																		<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.exportFilterExpressionJson')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			copyViewDraftFilterExpressionSummary(expressionSummaryText)}
																		disabled={!expressionSummaryText}
																		aria-label={$t('forms.copyFilterExpressionSummary')}
																		data-filter-expression-copy-summary
																		data-filter-expression-summary-copied={s.copiedFilterExpressionSummary ===
																		expressionSummaryText
																			? 'true'
																			: undefined}
																	>
																		<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.copyFilterExpressionSummary')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			exportViewDraftFilterExpressionSummary(expressionSummaryText)}
																		disabled={!expressionSummaryText}
																		aria-label={$t('forms.exportFilterExpressionSummary')}
																		data-filter-expression-export-summary
																		data-filter-expression-summary-exported={s.exportedFilterExpressionSummary ===
																		expressionSummaryText
																			? 'true'
																			: undefined}
																	>
																		<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.exportFilterExpressionSummary')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			focusViewDraftFilterExpressionRecords(
																				expressionMatchingRecords
																			)}
																		disabled={!expressionRecordIdsText}
																		data-filter-expression-focus-records
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.focusFilterExpressionRecords')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			exportViewDraftFilterExpressionCsv(expressionMatchingRecords)}
																		disabled={!expressionRecordIdsText}
																		data-filter-expression-export-csv
																		data-filter-expression-exported={s.exportedFilterExpressionRecordIds ===
																		expressionRecordIdsText
																			? 'true'
																			: undefined}
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.exportFilterExpressionRecords')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			copyViewDraftFilterExpressionCsvRecords(
																				expressionMatchingRecords
																			)}
																		disabled={!expressionRecordIdsText}
																		data-filter-expression-copy-matches-csv
																		data-filter-expression-matches-csv-copied={s.copiedFilterExpressionCsvRecordIds ===
																		expressionRecordIdsText
																			? 'true'
																			: undefined}
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.copyFilterExpressionRecordsCsv')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			copyViewDraftFilterExpressionJsonRecords(
																				expressionMatchingRecords
																			)}
																		disabled={!expressionRecordIdsText}
																		data-filter-expression-copy-matches-json
																		data-filter-expression-matches-json-copied={s.copiedFilterExpressionJsonRecordIds ===
																		expressionRecordIdsText
																			? 'true'
																			: undefined}
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.copyFilterExpressionRecordsJson')}
																	</button>

																	<button
																		type="button"
																		class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-2 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																		onclick={() =>
																			exportViewDraftFilterExpressionJson(
																				expressionMatchingRecords
																			)}
																		disabled={!expressionRecordIdsText}
																		data-filter-expression-export-json
																		data-filter-expression-json-exported={s.exportedFilterExpressionJsonRecordIds ===
																		expressionRecordIdsText
																			? 'true'
																			: undefined}
																		data-record-count={expressionMatchCount ?? ''}
																	>
																		<FileText class="h-3.5 w-3.5" aria-hidden="true" />
																		{$t('forms.exportFilterExpressionRecordsJson')}
																	</button>
																</div>
															{/if}

															{#each visibleViewDraftFilterGroups() as group, groupIndex}
																{@const groupMatchCount = viewDraftFilterGroupMatchCount(group)}

																<div
																	class={`space-y-2 rounded-card border border-line bg-raised p-2 ${
																		group.disabled ? 'opacity-70 ring-1 ring-warning' : ''
																	}`}
																	data-filter-group-disabled-state={group.disabled
																		? 'true'
																		: 'false'}
																>
																	<div class="flex flex-wrap items-end gap-2">
																		<label class="min-w-36 flex-1 space-y-1">
																			<span class="text-xs font-medium uppercase text-fg-muted"
																				>{$t('forms.filterGroup')} {groupIndex + 1}</span
																			>

																			<select
																				value={group.logic}
																				onchange={(event) =>
																					updateViewDraftFilterGroupLogic(
																						groupIndex,
																						selectValue(event) as ViewFilterLogic
																					)}
																				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																			>
																				<option value="all">{$t('forms.filterMatchAll')}</option>
																				<option value="any">{$t('forms.filterMatchAny')}</option>
																			</select>
																		</label>

																		<Button
																			size="sm"
																			variant="secondary"
																			onclick={() => addViewDraftFilter(groupIndex)}
																			disabled={hasReachedFilterLimit()}
																		>
																			<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
																			{$t('forms.addFilter')}
																		</Button>

																		<span
																			class="self-end rounded-control border border-line bg-nav px-2.5 py-2 text-xs font-medium text-fg-muted"
																			data-filter-group-match-count={groupIndex + 1}
																			data-record-count={groupMatchCount ?? ''}
																		>
																			{groupMatchCount === null
																				? $t('forms.filterMatchCountPending')
																				: $t('forms.filterMatchCount', {
																						values: { count: groupMatchCount }
																					})}
																		</span>

																		<div
																			class="flex items-center rounded-control border border-line bg-nav p-0.5"
																		>
																			<button
																				type="button"
																				class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg"
																				onclick={() =>
																					toggleViewDraftFilterGroupDisabled(groupIndex)}
																				aria-label={`${
																					group.disabled
																						? $t('forms.enableFilterGroup')
																						: $t('forms.disableFilterGroup')
																				} ${groupIndex + 1}`}
																				title={group.disabled
																					? $t('forms.enableFilterGroup')
																					: $t('forms.disableFilterGroup')}
																				data-filter-group-toggle-disabled={groupIndex + 1}
																				data-filter-group-disabled={group.disabled
																					? 'true'
																					: 'false'}
																			>
																				{#if group.disabled}
																					<Eye class="h-4 w-4" aria-hidden="true" />
																				{:else}
																					<EyeOff class="h-4 w-4" aria-hidden="true" />
																				{/if}
																			</button>

																			<button
																				type="button"
																				class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() =>
																					mergeViewDraftFilterGroupWithPrevious(groupIndex)}
																				disabled={!canMergeViewDraftFilterGroup(groupIndex)}
																				aria-label={`${$t('forms.mergeFilterGroup')} ${groupIndex + 1}`}
																				data-filter-group-merge-previous={groupIndex + 1}
																				><Link class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() => duplicateViewDraftFilterGroup(groupIndex)}
																				disabled={!canDuplicateViewDraftFilterGroup(groupIndex)}
																				aria-label={`${$t('forms.duplicateFilterGroup')} ${groupIndex + 1}`}
																				data-filter-group-duplicate={groupIndex + 1}
																				><Copy class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() => moveViewDraftFilterGroup(groupIndex, -1)}
																				disabled={groupIndex === 0}
																				aria-label={`${$t('forms.moveFilterGroupUp')} ${groupIndex + 1}`}
																				data-filter-group-move-up={groupIndex + 1}
																				><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="rounded p-1.5 text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() => moveViewDraftFilterGroup(groupIndex, 1)}
																				disabled={groupIndex >=
																					visibleViewDraftFilterGroups().length - 1}
																				aria-label={`${$t('forms.moveFilterGroupDown')} ${groupIndex + 1}`}
																				data-filter-group-move-down={groupIndex + 1}
																				><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
																			>
																		</div>

																		{#if s.viewDraft.filterGroups.length > 1}
																			<button
																				type="button"
																				class="rounded-control p-2 text-fg-muted hover:bg-surface hover:text-danger"
																				onclick={() => removeViewDraftFilterGroup(groupIndex)}
																				aria-label={$t('forms.removeFilterGroup')}
																				data-filter-group-remove={groupIndex + 1}
																				><X class="h-4 w-4" aria-hidden="true" /></button
																			>
																		{/if}
																	</div>

																	{#each group.filters as filter, filterIndex}
																		{@const filterMatchCount = viewDraftFilterMatchCount(filter)}

																		<div
																			class={`grid gap-2 rounded-card border border-line bg-nav p-2 sm:grid-cols-[minmax(0,1fr)_150px_minmax(0,1fr)_auto_auto_auto_auto_auto_auto] ${
																				filter.disabled ? 'opacity-70 ring-1 ring-warning' : ''
																			}`}
																			data-filter-disabled-state={filter.disabled
																				? 'true'
																				: 'false'}
																		>
																			<label class="space-y-1">
																				<span class="text-xs font-medium uppercase text-fg-muted"
																					>{$t('forms.filterField')}
																					{filterLabelSuffix(groupIndex, filterIndex)}</span
																				>

																				<select
																					value={filter.field}
																					onchange={(event) =>
																						updateViewDraftFilter(
																							filterIndex,
																							{ field: selectValue(event) },
																							groupIndex
																						)}
																					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																				>
																					<option value="">{$t('forms.viewNoRule')}</option>

																					{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
																						<option value={field.key}>{fieldLabel(field)}</option>
																					{/each}
																				</select>
																			</label>

																			<label class="space-y-1">
																				<span class="text-xs font-medium uppercase text-fg-muted"
																					>{$t('forms.filterOperator')}
																					{filterLabelSuffix(groupIndex, filterIndex)}</span
																				>

																				<select
																					value={filter.operator}
																					disabled={!filter.field}
																					onchange={(event) =>
																						updateViewDraftFilter(
																							filterIndex,
																							{
																								operator: selectValue(event) as ViewFilterOperator
																							},
																							groupIndex
																						)}
																					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																				>
																					<option value="contains"
																						>{$t('forms.filterContains')}</option
																					>
																					<option value="equals">{$t('forms.filterEquals')}</option>
																					<option value="not_equals"
																						>{$t('forms.filterNotEquals')}</option
																					>
																					<option value="not_empty"
																						>{$t('forms.filterNotEmpty')}</option
																					>
																				</select>
																			</label>

																			<label class="space-y-1">
																				<span class="text-xs font-medium uppercase text-fg-muted"
																					>{$t('forms.filterValue')}
																					{filterLabelSuffix(groupIndex, filterIndex)}</span
																				>

																				<input
																					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																					value={filter.value}
																					disabled={!filter.field ||
																						filter.operator === 'not_empty'}
																					oninput={(event) =>
																						updateViewDraftFilter(
																							filterIndex,
																							{ value: inputValue(event) },
																							groupIndex
																						)}
																				/>
																			</label>

																			<span
																				class="self-end rounded-control border border-line bg-raised px-2.5 py-2 text-xs font-medium text-fg-muted"
																				data-filter-match-count={`${groupIndex + 1}.${filterIndex + 1}`}
																				data-record-count={filterMatchCount ?? ''}
																			>
																				{filterMatchCount === null
																					? $t('forms.filterMatchCountPending')
																					: $t('forms.filterMatchCount', {
																							values: { count: filterMatchCount }
																						})}
																			</span>

																			<button
																				type="button"
																				class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg"
																				onclick={() =>
																					toggleViewDraftFilterDisabled(groupIndex, filterIndex)}
																				aria-label={`${filter.disabled ? $t('forms.enableFilter') : $t('forms.disableFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																				title={filter.disabled
																					? $t('forms.enableFilter')
																					: $t('forms.disableFilter')}
																				data-filter-toggle-disabled={`${groupIndex + 1}.${filterIndex + 1}`}
																				data-filter-disabled={filter.disabled ? 'true' : 'false'}
																			>
																				{#if filter.disabled}
																					<Eye class="h-4 w-4" aria-hidden="true" />
																				{:else}
																					<EyeOff class="h-4 w-4" aria-hidden="true" />
																				{/if}
																			</button>

																			<button
																				type="button"
																				class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-danger"
																				onclick={() =>
																					removeViewDraftFilter(filterIndex, groupIndex)}
																				aria-label={$t('forms.removeFilter')}
																				data-filter-remove={`${groupIndex + 1}.${filterIndex + 1}`}
																				><X class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() =>
																					duplicateViewDraftFilter(groupIndex, filterIndex)}
																				disabled={hasReachedFilterLimit()}
																				aria-label={`${$t('forms.duplicateFilter')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																				data-filter-duplicate={`${groupIndex + 1}.${filterIndex + 1}`}
																				><Copy class="h-4 w-4" aria-hidden="true" /></button
																			>

																			<button
																				type="button"
																				class="self-end rounded-control p-2 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																				onclick={() =>
																					splitViewDraftFilterToGroup(groupIndex, filterIndex)}
																				disabled={!canSplitViewDraftFilterToGroup(
																					groupIndex,
																					filterIndex
																				)}
																				aria-label={`${$t('forms.splitFilterToGroup')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																				data-filter-split-group={`${groupIndex + 1}.${filterIndex + 1}`}
																				><ArrowLeftRight
																					class="h-4 w-4"
																					aria-hidden="true"
																				/></button
																			>

																			<div
																				class="self-end flex items-center rounded-control border border-line bg-raised p-0.5"
																			>
																				<button
																					type="button"
																					class="rounded p-1.5 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					onclick={() =>
																						moveViewDraftFilter(groupIndex, filterIndex, -1)}
																					disabled={filterIndex === 0}
																					aria-label={`${$t('forms.moveFilterUp')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																					data-filter-move-up={`${groupIndex + 1}.${filterIndex + 1}`}
																					><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
																				>

																				<button
																					type="button"
																					class="rounded p-1.5 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					onclick={() =>
																						moveViewDraftFilter(groupIndex, filterIndex, 1)}
																					disabled={filterIndex >= group.filters.length - 1}
																					aria-label={`${$t('forms.moveFilterDown')} ${filterLabelSuffix(groupIndex, filterIndex)}`}
																					data-filter-move-down={`${groupIndex + 1}.${filterIndex + 1}`}
																					><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
																				>
																			</div>
																		</div>
																	{/each}
																</div>
															{/each}
														</div>

														<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.sortField')}</span
																>

																<select
																	value={s.viewDraft.sortField}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			sortField: selectValue(event)
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																>
																	<option value="">{$t('forms.viewNoRule')}</option>

																	{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
																		<option value={field.key}>{fieldLabel(field)}</option>
																	{/each}
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.sortDirection')}</span
																>

																<select
																	value={s.viewDraft.sortDirection}
																	disabled={!s.viewDraft.sortField}
																	onchange={(event) =>
																		(s.viewDraft = {
																			...s.viewDraft,
																			sortDirection: selectValue(event) as ViewSortDirection
																		})}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	<option value="asc">{$t('forms.sortAscending')}</option>
																	<option value="desc">{$t('forms.sortDescending')}</option>
																</select>
															</label>

															<label class="space-y-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.groupField')}</span
																>

																<select
																	value={s.viewDraft.groupBy}
																	onchange={(event) => {
																		const groupBy = selectValue(event);

																		s.viewDraft = {
																			...s.viewDraft,
																			groupBy,
																			swimlaneBy:
																				s.viewDraft.swimlaneBy === groupBy
																					? ''
																					: s.viewDraft.swimlaneBy,
																			wipLimits: {}
																		};
																	}}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																>
																	<option value="">{$t('forms.viewNoRule')}</option>

																	{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
																		<option value={field.key}>{fieldLabel(field)}</option>
																	{/each}
																</select>
															</label>

															{#if s.viewDraft.view_type === 'pivot'}
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotRowField')}</span
																	>

																	<select
																		value={s.viewDraft.pivotRowField}
																		onchange={(event) => {
																			const pivotRowField = selectValue(event);

																			s.viewDraft = {
																				...s.viewDraft,
																				pivotRowField,
																				pivotColumnField:
																					s.viewDraft.pivotColumnField === pivotRowField
																						? ''
																						: s.viewDraft.pivotColumnField
																			};
																		}}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftPivotDimensionFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotColumnField')}</span
																	>

																	<select
																		value={s.viewDraft.pivotColumnField}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotColumnField: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftPivotDimensionFieldCandidates().filter((field) => field.key !== s.viewDraft.pivotRowField) as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotValueField')}</span
																	>

																	<select
																		value={s.viewDraft.pivotValueField}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotValueField: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftPivotValueFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotAggregate')}</span
																	>

																	<select
																		value={s.viewDraft.pivotAggregate}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotAggregate: viewPivotAggregate(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="sum">{$t('forms.pivotAggregateSum')}</option>
																		<option value="count">{$t('forms.pivotAggregateCount')}</option>
																		<option value="avg">{$t('forms.pivotAggregateAvg')}</option>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotTotals')}</span
																	>

																	<select
																		value={s.viewDraft.pivotTotals}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotTotals: viewPivotTotalsMode(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="all">{$t('forms.pivotTotalsAll')}</option>
																		<option value="rows">{$t('forms.pivotTotalsRows')}</option>
																		<option value="columns">{$t('forms.pivotTotalsColumns')}</option
																		>
																		<option value="none">{$t('forms.pivotTotalsNone')}</option>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotValueDisplay')}</span
																	>

																	<select
																		value={s.viewDraft.pivotValueDisplay}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotValueDisplay: viewPivotValueDisplay(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="value"
																			>{$t('forms.pivotValueDisplayValue')}</option
																		>
																		<option value="percent_row"
																			>{$t('forms.pivotValueDisplayPercentRow')}</option
																		>
																		<option value="percent_column"
																			>{$t('forms.pivotValueDisplayPercentColumn')}</option
																		>
																		<option value="percent_total"
																			>{$t('forms.pivotValueDisplayPercentTotal')}</option
																		>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotSort')}</span
																	>

																	<select
																		value={s.viewDraft.pivotSort}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotSort: viewPivotSortMode(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="label">{$t('forms.pivotSortLabel')}</option>
																		<option value="row_total_desc"
																			>{$t('forms.pivotSortRowTotalDesc')}</option
																		>
																		<option value="row_total_asc"
																			>{$t('forms.pivotSortRowTotalAsc')}</option
																		>
																		<option value="column_total_desc"
																			>{$t('forms.pivotSortColumnTotalDesc')}</option
																		>
																		<option value="column_total_asc"
																			>{$t('forms.pivotSortColumnTotalAsc')}</option
																		>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotRowLimit')}</span
																	>

																	<select
																		value={String(s.viewDraft.pivotRowLimit)}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotRowLimit: viewPivotRowLimit(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="0">{$t('forms.pivotRowLimitAll')}</option>
																		<option value="2">{$t('forms.pivotRowLimitTwo')}</option>
																		<option value="5">{$t('forms.pivotRowLimitFive')}</option>
																		<option value="10">{$t('forms.pivotRowLimitTen')}</option>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.pivotColumnLimit')}</span
																	>

																	<select
																		value={String(s.viewDraft.pivotColumnLimit)}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotColumnLimit: viewPivotColumnLimit(selectValue(event))
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="0">{$t('forms.pivotColumnLimitAll')}</option>
																		<option value="2">{$t('forms.pivotColumnLimitTwo')}</option>
																		<option value="5">{$t('forms.pivotColumnLimitFive')}</option>
																		<option value="10">{$t('forms.pivotColumnLimitTen')}</option>
																	</select>
																</label>

																<label
																	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-input accent-primary"
																		checked={s.viewDraft.pivotEmptyBuckets}
																		data-pivot-empty-buckets-control
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotEmptyBuckets:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>

																	<span class="min-w-0 truncate"
																		>{$t('forms.pivotEmptyBuckets')}</span
																	>
																</label>

																<label
																	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-input accent-primary"
																		checked={s.viewDraft.pivotHideZeroBuckets}
																		data-pivot-hide-zero-buckets-control
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotHideZeroBuckets:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>

																	<span class="min-w-0 truncate"
																		>{$t('forms.pivotHideZeroBuckets')}</span
																	>
																</label>

																<label
																	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-input accent-primary"
																		checked={s.viewDraft.pivotShowCounts}
																		data-pivot-show-counts-control
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotShowCounts:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>

																	<span class="min-w-0 truncate">{$t('forms.pivotShowCounts')}</span
																	>
																</label>

																<label
																	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-input accent-primary"
																		checked={s.viewDraft.pivotHeatmap}
																		data-pivot-heatmap-control
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				pivotHeatmap:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>

																	<span class="min-w-0 truncate">{$t('forms.pivotHeatmap')}</span>
																</label>

																<div class="flex items-end">
																	<button
																		type="button"
																		disabled={!s.viewDraft.pivotRowField ||
																			!s.viewDraft.pivotColumnField}
																		class="inline-flex h-10 items-center gap-2 rounded-control border border-input bg-raised px-3 text-sm font-medium text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
																		data-pivot-swap-axes
																		onclick={swapViewDraftPivotAxes}
																	>
																		<ArrowLeftRight class="h-4 w-4" aria-hidden="true" />
																		{$t('forms.pivotSwapAxes')}
																	</button>
																</div>
															{/if}

															{#if s.viewDraft.view_type === 'gantt'}
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.ganttStartField')}</span
																	>

																	<select
																		value={s.viewDraft.ganttStartField}
																		onchange={(event) => {
																			const ganttStartField = selectValue(event);

																			s.viewDraft = {
																				...s.viewDraft,
																				ganttStartField,
																				ganttEndField:
																					s.viewDraft.ganttEndField === ganttStartField
																						? ''
																						: s.viewDraft.ganttEndField,
																				ganttGroupBy:
																					s.viewDraft.ganttGroupBy === ganttStartField
																						? ''
																						: s.viewDraft.ganttGroupBy,
																				ganttDependencyField:
																					s.viewDraft.ganttDependencyField === ganttStartField
																						? ''
																						: s.viewDraft.ganttDependencyField
																			};
																		}}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftGanttDateFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.ganttEndField')}</span
																	>

																	<select
																		value={s.viewDraft.ganttEndField}
																		onchange={(event) => {
																			const ganttEndField = selectValue(event);

																			s.viewDraft = {
																				...s.viewDraft,
																				ganttEndField,
																				ganttGroupBy:
																					s.viewDraft.ganttGroupBy === ganttEndField
																						? ''
																						: s.viewDraft.ganttGroupBy,
																				ganttDependencyField:
																					s.viewDraft.ganttDependencyField === ganttEndField
																						? ''
																						: s.viewDraft.ganttDependencyField
																			};
																		}}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.ganttNoEndField')}</option>

																		{#each viewDraftGanttDateFieldCandidates().filter((field) => field.key !== s.viewDraft.ganttStartField) as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.ganttGroupField')}</span
																	>

																	<select
																		value={s.viewDraft.ganttGroupBy}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				ganttGroupBy: selectValue(event),
																				ganttDependencyField:
																					s.viewDraft.ganttDependencyField === selectValue(event)
																						? ''
																						: s.viewDraft.ganttDependencyField
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftGanttGroupFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.ganttDependencyField')}</span
																	>

																	<select
																		value={s.viewDraft.ganttDependencyField}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				ganttDependencyField: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftGanttDependencyFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.ganttScale')}</span
																	>

																	<select
																		value={s.viewDraft.ganttScale}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				ganttScale: selectValue(event) as GanttScale
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="day">{$t('forms.ganttScaleDay')}</option>
																		<option value="week">{$t('forms.ganttScaleWeek')}</option>
																		<option value="month">{$t('forms.ganttScaleMonth')}</option>
																	</select>
																</label>
															{/if}

															{#if s.viewDraft.view_type === 'kanban'}
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.swimlaneField')}</span
																	>

																	<select
																		value={s.viewDraft.swimlaneBy}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				swimlaneBy: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftSwimlaneFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>
															{/if}

															<label class="space-y-1 sm:col-span-2 xl:col-span-1">
																<span class="text-xs font-medium uppercase text-fg-muted"
																	>{$t('forms.dateField')}</span
																>

																<select
																	value={s.viewDraft.dateField}
																	onchange={(event) => {
																		const dateField = selectValue(event);

																		s.viewDraft = {
																			...s.viewDraft,
																			dateField,
																			calendarEndField:
																				s.viewDraft.calendarEndField === dateField
																					? ''
																					: s.viewDraft.calendarEndField,
																			calendarResourceBy:
																				s.viewDraft.calendarResourceBy === dateField
																					? ''
																					: s.viewDraft.calendarResourceBy
																		};
																	}}
																	class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																>
																	<option value="">{$t('forms.viewNoRule')}</option>

																	{#each s.fields.filter( (field) => ['date', 'datetime'].includes(field.type ?? 'text') ) as field}
																		<option value={field.key}>{fieldLabel(field)}</option>
																	{/each}
																</select>
															</label>

															{#if s.viewDraft.view_type === 'calendar'}
																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarEndField')}</span
																	>

																	<select
																		value={s.viewDraft.calendarEndField}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarEndField: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftCalendarEndFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarResourceField')}</span
																	>

																	<select
																		value={s.viewDraft.calendarResourceBy}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarResourceBy: selectValue(event)
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="">{$t('forms.viewNoRule')}</option>

																		{#each viewDraftCalendarResourceFieldCandidates() as field}
																			<option value={field.key}>{fieldLabel(field)}</option>
																		{/each}
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarRange')}</span
																	>

																	<select
																		value={s.viewDraft.calendarRange}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarRange: selectValue(event) as ViewCalendarRange
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="week">{$t('forms.calendarRangeWeek')}</option>
																		<option value="month">{$t('forms.calendarRangeMonth')}</option>
																	</select>
																</label>

																<div class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarFocusDate')}</span
																	>

																	<div class="grid grid-cols-[auto_minmax(0,1fr)_auto] gap-1">
																		<button
																			type="button"
																			class="inline-flex h-9 w-9 items-center justify-center rounded-control border border-input text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																			onclick={() => shiftViewDraftCalendarFocus(-1)}
																			aria-label={$t('forms.calendarPreviousRange')}
																			title={$t('forms.calendarPreviousRange')}
																			data-calendar-focus-shift="-1"
																			><ChevronLeft class="h-4 w-4" aria-hidden="true" /></button
																		>

																		<input
																			type="date"
																			value={s.viewDraft.calendarFocusDate}
																			aria-label={$t('forms.calendarFocusDate')}
																			data-calendar-focus-date-control
																			oninput={(event) =>
																				(s.viewDraft = {
																					...s.viewDraft,
																					calendarFocusDate: inputValue(event)
																				})}
																			class="min-w-0 rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																		/>

																		<button
																			type="button"
																			class="inline-flex h-9 w-9 items-center justify-center rounded-control border border-input text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																			onclick={() => shiftViewDraftCalendarFocus(1)}
																			aria-label={$t('forms.calendarNextRange')}
																			title={$t('forms.calendarNextRange')}
																			data-calendar-focus-shift="1"
																			><ChevronRight class="h-4 w-4" aria-hidden="true" /></button
																		>
																	</div>
																</div>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarRecurrence')}</span
																	>

																	<select
																		value={s.viewDraft.calendarRecurrence}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarRecurrence: selectValue(
																					event
																				) as ViewCalendarRecurrence
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="none"
																			>{$t('forms.calendarRecurrenceNone')}</option
																		>
																		<option value="daily"
																			>{$t('forms.calendarRecurrenceDaily')}</option
																		>
																		<option value="weekly"
																			>{$t('forms.calendarRecurrenceWeekly')}</option
																		>
																		<option value="monthly"
																			>{$t('forms.calendarRecurrenceMonthly')}</option
																		>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarDayScope')}</span
																	>

																	<select
																		value={s.viewDraft.calendarDayScope}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarDayScope: selectValue(event) as ViewCalendarDayScope
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="all">{$t('forms.calendarDayScopeAll')}</option>
																		<option value="workweek"
																			>{$t('forms.calendarDayScopeWorkweek')}</option
																		>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.calendarLayout')}</span
																	>

																	<select
																		value={s.viewDraft.calendarLayout}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarLayout: selectValue(event) as ViewCalendarLayout
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="grid">{$t('forms.calendarLayoutGrid')}</option>
																		<option value="agenda"
																			>{$t('forms.calendarLayoutAgenda')}</option
																		>
																	</select>
																</label>

																<label
																	class="flex items-center gap-2 rounded-control border border-line bg-nav px-3 py-2 text-sm text-fg"
																>
																	<input
																		type="checkbox"
																		class="h-4 w-4 rounded border-input accent-primary"
																		checked={s.viewDraft.calendarHideEmptyDays}
																		data-calendar-hide-empty-days-control
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				calendarHideEmptyDays:
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																			})}
																	/>

																	<span class="min-w-0 truncate"
																		>{$t('forms.calendarHideEmptyDays')}</span
																	>
																</label>
															{/if}

															<div
																class="space-y-2 rounded-card border border-line p-3 lg:col-span-2"
															>
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
																			<span class="text-xs font-medium uppercase text-fg-muted"
																				>{$t('forms.cardFieldLimit')}</span
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

																			<span class="min-w-0 truncate"
																				>{$t('forms.cardHideEmptyFields')}</span
																			>
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

																			<span class="min-w-0 truncate"
																				>{$t('forms.cardShowTimestamps')}</span
																			>
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

																			<span class="min-w-0 truncate"
																				>{$t('forms.cardShowRecordId')}</span
																			>
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
																					<span class="truncate"
																						>{exception.label} · {exception.date}</span
																					>

																					<button
																						type="button"
																						class="rounded p-0.5 text-fg-muted hover:bg-selected hover:text-fg"
																						onclick={() =>
																							removeViewDraftCalendarException(
																								exception.recordId,
																								exception.date
																							)}
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
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.cardLayout')}</span
																	>

																	<select
																		value={s.viewDraft.cardLayout}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				cardLayout: selectValue(event) as CardLayoutMode
																			})}
																		class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg disabled:bg-surface disabled:text-fg-muted"
																	>
																		<option value="standard"
																			>{$t('forms.cardLayoutStandard')}</option
																		>
																		<option value="gallery">{$t('forms.cardLayoutGallery')}</option>
																		<option value="compact">{$t('forms.cardLayoutCompact')}</option>
																		<option value="list">{$t('forms.cardLayoutList')}</option>
																	</select>
																</label>

																<label class="space-y-1">
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.cardTitleField')}</span
																	>

																	<select
																		value={s.viewDraft.cardTitleField}
																		onchange={(event) => {
																			const cardTitleField = selectValue(event);

																			s.viewDraft = {
																				...s.viewDraft,
																				cardTitleField,
																				cardSubtitleField:
																					s.viewDraft.cardSubtitleField === cardTitleField
																						? ''
																						: s.viewDraft.cardSubtitleField,
																				cardBadgeField:
																					s.viewDraft.cardBadgeField === cardTitleField
																						? ''
																						: s.viewDraft.cardBadgeField,
																				cardFieldKeys:
																					cardTitleField &&
																					s.viewDraft.cardFieldKeys[0] !== cardTitleField
																						? [
																								cardTitleField,
																								...s.viewDraft.cardFieldKeys.filter(
																									(key) => key !== cardTitleField
																								)
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
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.cardSubtitleField')}</span
																	>

																	<select
																		value={s.viewDraft.cardSubtitleField}
																		onchange={(event) =>
																			(s.viewDraft = {
																				...s.viewDraft,
																				cardSubtitleField: selectValue(event),
																				cardBadgeField:
																					s.viewDraft.cardBadgeField === selectValue(event)
																						? ''
																						: s.viewDraft.cardBadgeField
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
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.cardBadgeField')}</span
																	>

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
																	<span class="text-xs font-medium uppercase text-fg-muted"
																		>{$t('forms.cardCoverField')}</span
																	>

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
														</div>

														{#if s.viewDraft.view_type === 'card' && viewDraftCardFields().length > 0}
															<div class="space-y-2 rounded-card border border-line bg-raised p-3">
																<div>
																	<h5 class="text-xs font-semibold uppercase text-fg-muted">
																		{$t('forms.cardFieldOrder')}
																	</h5>
																</div>

																<div class="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
																	{#each viewDraftCardFields() as field, fieldIndex}
																		<div
																			class="flex min-w-0 items-center justify-between gap-2 rounded-control border border-line bg-nav px-2 py-1.5 text-sm"
																			data-card-field-order={`${fieldIndex + 1}:${field.key}`}
																		>
																			<span class="min-w-0 truncate text-fg"
																				>{fieldIndex + 1}. {fieldLabel(field)}</span
																			>

																			<div
																				class="flex shrink-0 items-center rounded-control border border-line bg-raised p-0.5"
																			>
																				<button
																					type="button"
																					class="rounded p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					disabled={fieldIndex === 0}
																					onclick={() => moveViewDraftCardField(fieldIndex, -1)}
																					aria-label={`${$t('forms.moveCardFieldUp')} ${fieldLabel(field)}`}
																					data-card-field-move-up={field.key}
																					><ArrowUp
																						class="h-3.5 w-3.5"
																						aria-hidden="true"
																					/></button
																				>

																				<button
																					type="button"
																					class="rounded p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					disabled={fieldIndex >= viewDraftCardFields().length - 1}
																					onclick={() => moveViewDraftCardField(fieldIndex, 1)}
																					aria-label={`${$t('forms.moveCardFieldDown')} ${fieldLabel(field)}`}
																					data-card-field-move-down={field.key}
																					><ArrowDown
																						class="h-3.5 w-3.5"
																						aria-hidden="true"
																					/></button
																				>
																			</div>
																		</div>
																	{/each}
																</div>
															</div>
														{/if}

														{#if viewDraftKanbanWipOptions().length > 0}
															<div class="space-y-3 rounded-card border border-line bg-raised p-3">
																<div>
																	<h5 class="text-xs font-semibold uppercase text-fg-muted">
																		{$t('forms.kanbanWipPolicy')}
																	</h5>

																	<p class="mt-1 text-xs text-fg-muted">
																		{$t('forms.kanbanWipPolicyDescription')}
																	</p>
																</div>

																<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
																	{#each viewDraftKanbanWipOptions() as option}
																		<label class="space-y-1">
																			<span class="text-xs font-medium uppercase text-fg-muted"
																				>{$t('forms.kanbanWipLimitFor', {
																					values: { option }
																				})}</span
																			>

																			<input
																				type="number"
																				min="0"
																				step="1"
																				inputmode="numeric"
																				placeholder={$t('forms.kanbanNoWipLimit')}
																				value={s.viewDraft.wipLimits[option] ?? ''}
																				oninput={(event) =>
																					setViewDraftWipLimit(option, inputValue(event))}
																				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																			/>
																		</label>
																	{/each}
																</div>
															</div>
														{/if}
													</div>
												</div>
											</div>

											<details
												class="rounded-card border border-line bg-raised"
												open
												data-view-columns-disclosure
											>
												<summary
													class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 marker:hidden"
												>
													<span class="text-sm font-semibold text-fg"
														>{$t('forms.visibleColumns')}</span
													>

													<span class="text-xs text-fg-muted"
														>{$t('forms.selectedColumns', {
															values: { count: s.viewDraft.columns.length }
														})}</span
													>
												</summary>

												<div class="border-t border-line p-3">
													<div class="mb-2 flex items-center justify-between gap-3">
														<span class="text-xs font-medium uppercase text-fg-muted"
															>{$t('forms.visibleColumns')}</span
														>

														<span class="text-xs text-fg-muted"
															>{$t('forms.selectedColumns', {
																values: { count: s.viewDraft.columns.length }
															})}</span
														>
													</div>

													<div class="grid gap-2 sm:grid-cols-2">
														{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
															<div
																class="flex items-center gap-2 rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg"
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-input accent-primary"
																	checked={viewDraftHasColumn(field.key)}
																	aria-label={`${$t('forms.visibleColumns')}: ${fieldLabel(field)}`}
																	onchange={(event) =>
																		setViewDraftColumn(
																			field.key,
																			event.currentTarget instanceof HTMLInputElement
																				? event.currentTarget.checked
																				: false
																		)}
																/>

																<span class="min-w-0 flex-1 truncate">{fieldLabel(field)}</span>

																{#if memberFieldHidden(field)}
																	<span
																		class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
																		data-view-column-permission-hidden={field.key}
																		>{$t('forms.fieldHiddenBadge')}</span
																	>
																{/if}

																{#if memberFieldLocked(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-permission-locked={field.key}
																		>{$t('forms.fieldLockedBadge')}</span
																	>
																{/if}

																{#if fieldHasConditionalHidden(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-conditional-hidden={field.key}
																		>{$t('forms.fieldConditionHiddenBadge')}</span
																	>
																{/if}

																{#if fieldHasConditionalReadOnly(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-conditional-readonly={field.key}
																		>{$t('forms.fieldConditionReadOnlyBadge')}</span
																	>
																{/if}

																<span
																	class="ml-auto hidden font-mono text-[11px] text-fg-muted 2xl:inline"
																	>{field.key}</span
																>
															</div>
														{/each}
													</div>
												</div>
											</details>
										</div>
									</section>
								</LayoutSettingsSheet>

								{#if s.recordEditorOpen}
									<div
										use:recordEditorAction
										class="pointer-events-auto absolute inset-0 z-30 flex justify-end bg-scrim"
										data-record-editor-drawer
										role="dialog"
										aria-modal="false"
										aria-labelledby="record-editor-title"
									>
										<button
											type="button"
											class="absolute inset-0 cursor-default"
											aria-hidden="true"
											tabindex="-1"
											onclick={closeRecordEditorDrawer}
										></button>

										<section
											bind:this={s.recordEditorPanel}
											class="relative z-10 flex h-full w-full max-w-3xl flex-col overflow-y-auto bg-overlay p-4 shadow-sheet"
										>
											<div class="mb-3 flex items-center justify-between gap-3">
												<div>
													<h3 id="record-editor-title" class="text-base font-semibold text-fg">
														{s.editingRecordId ? $t('forms.editRecord') : $t('forms.newRecord')}
													</h3>

													<span class="text-xs text-fg-muted">{$t('forms.decimalHint')}</span>
												</div>

												<Button size="sm" variant="secondary" onclick={closeRecordEditorDrawer}>
													<X class="mr-2 h-4 w-4" aria-hidden="true" />
													{$t('common.cancel')}
												</Button>
											</div>

											<FormsSessionNotice />
											<div class="grid gap-3 md:grid-cols-2">
												<label class="space-y-1 md:col-span-2">
													<span class="text-sm font-medium text-fg"
														>{$t('forms.titleOverride')}</span
													>

													<input
														class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
														bind:value={s.recordTitle}
														placeholder={s.selectedForm.title_template}
													/>
												</label>

												{#each s.fields as field}
													<label
														class="space-y-1 {fieldType(field) === 'textarea' ||
														fieldType(field) === 'rich_text' ||
														fieldType(field) === 'signature' ||
														fieldType(field) === 'child_table'
															? 'md:col-span-2'
															: ''} {fieldHiddenByCondition(field, s.recordDraft) ? 'hidden' : ''}"
														data-record-edit-field={field.key}
														data-conditional-hidden={fieldHiddenByCondition(field, s.recordDraft)
															? field.key
															: undefined}
														data-conditional-readonly={fieldReadOnlyByCondition(
															field,
															s.recordDraft
														)
															? field.key
															: undefined}
													>
														<span
															class="flex flex-wrap items-center gap-2 text-sm font-medium text-fg"
														>
															<span>{fieldLabel(field)}</span>

															{#if field.required}
																<span class="text-danger">*</span>
															{/if}

															{#if memberFieldHidden(field)}
																<span
																	class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[11px] font-medium uppercase text-warning"
																	data-record-edit-permission-hidden={field.key}
																	>{$t('forms.fieldHiddenBadge')}</span
																>
															{/if}

															{#if memberFieldLocked(field)}
																<span
																	class="rounded-control bg-surface px-1.5 py-0.5 text-[11px] font-medium uppercase text-fg-muted"
																	data-record-edit-permission-locked={field.key}
																	>{$t('forms.fieldLockedBadge')}</span
																>
															{/if}
														</span>

														{#if fieldType(field) === 'boolean'}
															<input
																type="checkbox"
																checked={Boolean(s.recordDraft[field.key])}
																disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
																onchange={(event) =>
																	setDraftValue(
																		field.key,
																		event.currentTarget instanceof HTMLInputElement
																			? event.currentTarget.checked
																			: false
																	)}
																class="h-5 w-5 rounded border-input accent-primary"
															/>
														{:else if fieldType(field) === 'single_select'}
															<select
																value={draftString(field.key)}
																disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
																onchange={(event) => setDraftValue(field.key, selectValue(event))}
																class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
															>
																<option value="">{$t('forms.selectPlaceholder')}</option>

																{#each recordSelectOptions(field, s.recordDraft) as option}
																	<option
																		value={option}
																		disabled={optionDisabledForSingleDraft(field, option)}
																		data-option-disabled={optionDisabled(field, option)
																			? option
																			: undefined}
																		data-option-archived={optionArchived(field, option)
																			? option
																			: undefined}
																		data-option-description={optionDescription(field, option) ||
																			undefined}
																		data-option-group={optionGroup(field, option) || undefined}
																		>{optionDisplayLabel(field, option)}</option
																	>
																{/each}
															</select>
														{:else if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
															<div
																class="grid gap-2 rounded-control border border-input bg-field p-2 text-sm"
															>
																{#each recordSelectOptions(field, s.recordDraft) as option}
																	<label
																		class="flex items-start gap-2 text-fg"
																		data-option-description={optionDescription(field, option) ||
																			undefined}
																		data-option-group={optionGroup(field, option) || undefined}
																	>
																		<input
																			type="checkbox"
																			class="h-4 w-4 rounded border-input accent-primary"
																			checked={isDraftOptionSelected(field.key, option)}
																			disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
																				optionDisabledForMultiDraft(field, option)}
																			data-option-disabled={optionDisabled(field, option)
																				? option
																				: undefined}
																			data-option-archived={optionArchived(field, option)
																				? option
																				: undefined}
																			onchange={(event) =>
																				toggleDraftOption(
																					field.key,
																					option,
																					event.currentTarget instanceof HTMLInputElement
																						? event.currentTarget.checked
																						: false
																				)}
																		/>

																		<span class="min-w-0">
																			<span class="block">{optionDisplayLabel(field, option)}</span>

																			{#if optionDescription(field, option)}
																				<span class="block text-xs text-fg-muted"
																					>{optionDescription(field, option)}</span
																				>
																			{/if}
																		</span>
																	</label>
																{/each}
															</div>
														{:else if fieldType(field) === 'relation' && field.relation?.form_key}
															<select
																value={relationDraftValue(field)}
																disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
																	s.relationTargetsLoading}
																onchange={(event) => setDraftValue(field.key, selectValue(event))}
																class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
															>
																<option value="">
																	{s.relationTargetsLoading
																		? $t('forms.loadingRelationTargets')
																		: $t('forms.selectRelationTarget')}
																</option>

																{#each relationTargetOptions(field) as target}
																	<option value={target.record_id}
																		>{target.display || target.title}</option
																	>
																{/each}
															</select>
														{:else if fieldType(field) === 'child_table'}
															<div
																class="rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg-muted"
																data-child-table-field={field.key}
															>
																{$t('forms.childRecords')}
															</div>
														{:else if fieldType(field) === 'member'}
															<select
																value={draftString(field.key)}
																disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
																	s.workspaceMembersLoading}
																onchange={(event) => setDraftValue(field.key, selectValue(event))}
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
														{:else if ['attachment', 'image'].includes(fieldType(field))}
															<div class="space-y-2" data-attachment-field={field.key}>
																<div class="flex gap-2">
																	<input
																		type="url"
																		class="min-w-0 flex-1 rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																		value={attachmentUrl(field)}
																		placeholder={fieldPlaceholder(field) ||
																			$t('forms.attachmentUrlPlaceholder')}
																		readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
																		oninput={(event) => setDraftValue(field.key, inputValue(event))}
																		data-attachment-url-input={field.key}
																	/>

																	<Button
																		size="sm"
																		variant="secondary"
																		disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
																			!s.editingRecordId ||
																			!attachmentUrl(field)}
																		loading={s.attachmentSubmittingField === field.key}
																		onclick={() => createAttachmentForField(field)}
																	>
																		<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
																		{$t('forms.attachmentLink')}
																	</Button>
																</div>

																<div class="flex flex-wrap items-center gap-2">
																	<label
																		class="inline-flex cursor-pointer items-center rounded-control border border-input px-3 py-1.5 text-sm font-medium text-fg hover:bg-surface"
																	>
																		<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
																		{$t('forms.attachmentChooseFile')}

																		<input
																			type="file"
																			class="sr-only"
																			accept={attachmentAcceptAttr(field)}
																			disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
																				!s.editingRecordId}
																			onchange={(event) =>
																				createAttachmentFromLocalFile(field, event)}
																			data-attachment-file-input={field.key}
																		/>
																	</label>

																	{#if field.attachment?.max_size_mb}
																		<span class="text-xs text-fg-muted"
																			>{$t('forms.attachmentMaxSizeLabel', {
																				values: { size: field.attachment.max_size_mb }
																			})}</span
																		>
																	{/if}
																</div>

																{#if !s.editingRecordId}
																	<p class="text-xs text-fg-muted">
																		{$t('forms.attachmentSaveRecordFirst')}
																	</p>
																{/if}

																{#if attachmentPreviewUrl(field) && fieldType(field) === 'image'}
																	<img
																		src={attachmentPreviewUrl(field)}
																		alt={fieldLabel(field)}
																		class="h-24 w-full rounded-card border border-line object-cover"
																		data-attachment-preview={field.key}
																	/>
																{/if}

																{#if fieldAttachments(field.key).length > 0}
																	<div class="space-y-2">
																		{#each fieldAttachments(field.key) as attachment}
																			<div
																				class="flex flex-wrap items-center gap-2 rounded-card border border-line bg-nav px-3 py-2 text-xs"
																				data-attachment-item={attachment.id}
																			>
																				{#if attachment.url && attachmentIsPreviewableImage(attachment)}
																					<img
																						src={attachmentThumbnailUrl(attachment)}
																						alt={attachment.file_name}
																						class="h-12 w-12 shrink-0 rounded border border-line object-cover"
																						data-attachment-thumbnail={attachment.id}
																					/>
																				{:else}
																					<FileText
																						class="h-4 w-4 shrink-0 text-fg-muted"
																						aria-hidden="true"
																					/>
																				{/if}

																				<div class="min-w-0 flex-1">
																					<div class="truncate font-medium text-fg">
																						{attachment.file_name}
																					</div>

																					<div class="truncate font-mono text-[11px] text-fg-muted">
																						{attachment.content_type || attachment.storage_key}
																					</div>
																				</div>

																				{#if attachmentSignedUrl(attachment)}
																					<a
																						href={attachmentSignedUrl(attachment) || undefined}
																						target="_blank"
																						rel="noreferrer"
																						class="inline-flex items-center rounded-control px-2 py-1 text-xs font-medium text-fg underline hover:bg-surface"
																						data-attachment-open={attachment.id}
																						data-attachment-signed-url={attachmentSignedUrl(
																							attachment
																						)}>{$t('forms.attachmentOpen')}</a
																					>

																					<a
																						href={attachmentSignedUrl(attachment) || undefined}
																						download={attachment.file_name}
																						class="inline-flex items-center rounded-control px-2 py-1 text-xs font-medium text-fg hover:bg-surface"
																						data-attachment-download={attachment.id}
																						data-attachment-signed-download={attachmentSignedUrl(
																							attachment
																						)}
																					>
																						<FileDown class="mr-1 h-3.5 w-3.5" aria-hidden="true" />
																						{$t('forms.attachmentDownload')}
																					</a>
																				{/if}

																				<Button
																					size="sm"
																					variant="danger"
																					disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
																					loading={s.deletingAttachmentId === attachment.id}
																					onclick={() => removeAttachment(field, attachment)}
																					>{$t('forms.attachmentRemove')}</Button
																				>
																			</div>
																		{/each}
																	</div>
																{/if}
															</div>
														{:else if fieldType(field) === 'textarea' || fieldType(field) === 'rich_text'}
															<textarea
																class="min-h-24 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																value={draftString(field.key)}
																placeholder={fieldPlaceholder(field)}
																readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
																maxlength={field.validation?.max_length}
																oninput={(event) => setDraftValue(field.key, inputValue(event))}
															></textarea>
														{:else if fieldType(field) === 'signature'}
															<div class="space-y-2" data-signature-field={field.key}>
																<canvas
																	class="h-28 w-full touch-none rounded-card border border-dashed border-input bg-white"
																	data-signature-canvas={field.key}
																	aria-label={$t('forms.signatureCanvas')}
																	onpointerdown={(event) =>
																		startSignatureStroke(
																			event,
																			field.key,
																			'record',
																			fieldReadOnlyByCondition(field, s.recordDraft)
																		)}
																	onpointermove={(event) =>
																		moveSignatureStroke(
																			event,
																			field.key,
																			'record',
																			fieldReadOnlyByCondition(field, s.recordDraft)
																		)}
																	onpointerup={(event) =>
																		endSignatureStroke(
																			event,
																			field.key,
																			'record',
																			fieldReadOnlyByCondition(field, s.recordDraft)
																		)}
																	onpointercancel={(event) =>
																		endSignatureStroke(
																			event,
																			field.key,
																			'record',
																			fieldReadOnlyByCondition(field, s.recordDraft)
																		)}
																></canvas>

																<div class="flex flex-wrap items-center gap-2">
																	<Button
																		size="sm"
																		variant="secondary"
																		disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
																		onclick={() =>
																			clearSignatureCanvas(
																				field.key,
																				'record',
																				fieldReadOnlyByCondition(field, s.recordDraft)
																			)}>{$t('forms.signatureClear')}</Button
																	>

																	{#if signatureImageValue(s.recordDraft, field.key)}
																		<img
																			src={signatureImageValue(s.recordDraft, field.key)}
																			alt={fieldLabel(field)}
																			class="h-10 max-w-48 rounded border border-line bg-raised object-contain"
																			data-signature-preview={field.key}
																		/>
																	{/if}
																</div>

																<textarea
																	class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
																	value={draftString(field.key)}
																	placeholder={fieldPlaceholder(field)}
																	readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
																	maxlength={field.validation?.max_length}
																	oninput={(event) => setDraftValue(field.key, inputValue(event))}
																	data-signature-value={field.key}></textarea>
															</div>
														{:else if fieldType(field) === 'autonumber'}
															<input
																type="text"
																class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg-muted"
																value={draftString(field.key)}
																placeholder={$t('forms.autonumberGeneratedAfterSave')}
																readonly
																data-autonumber-field={field.key}
															/>
														{:else}
															<input
																type={fieldType(field) === 'date'
																	? 'date'
																	: fieldType(field) === 'datetime'
																		? 'datetime-local'
																		: fieldType(field) === 'email'
																			? 'email'
																			: fieldType(field) === 'phone'
																				? 'tel'
																				: [
																							'amount',
																							'number',
																							'integer',
																							'rating',
																							'progress'
																					  ].includes(fieldType(field))
																					? 'number'
																					: 'text'}
																step={['integer', 'rating', 'progress'].includes(fieldType(field))
																	? '1'
																	: ['amount', 'number'].includes(fieldType(field))
																		? '0.01'
																		: undefined}
																class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
																value={draftString(field.key)}
																placeholder={fieldPlaceholder(field)}
																readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
																min={field.validation?.min}
																max={field.validation?.max}
																pattern={field.validation?.pattern}
																maxlength={field.validation?.max_length}
																oninput={(event) => setDraftValue(field.key, inputValue(event))}
															/>
														{/if}

														{#if field.help_text}
															<span class="block text-xs text-fg-muted">{field.help_text}</span>
														{/if}

														{#if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
															<span class="block text-xs text-fg-muted"
																>{$t('forms.options', {
																	values: { options: fieldOptions(field).join(', ') }
																})}</span
															>
														{/if}
													</label>
												{/each}
											</div>

											<div class="mt-4 flex justify-end">
												<Button loading={s.recordSubmitting} onclick={handleSaveRecord}>
													{#if s.editingRecordId}
														<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
														{$t('forms.saveRecord')}
													{:else}
														<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
														{$t('forms.addRecord')}
													{/if}
												</Button>
											</div>
										</section>
									</div>
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
									{:else if s.selectedViewType === 'pivot'}
										<div class="overflow-x-auto p-4" data-view-renderer="pivot">
											{#if !s.pivotTable.rowField || !s.pivotTable.columnField || !s.pivotTable.valueField}
												<div
													class="rounded-card border border-dashed border-input bg-nav px-4 py-6 text-sm text-fg-muted"
												>
													{$t('forms.pivotConfigureFields')}
												</div>
											{:else}
												<div class="mb-3 flex flex-wrap items-center gap-2 text-xs text-fg-muted">
													<span data-pivot-row-field={s.pivotTable.rowField.key}
														>{$t('forms.pivotRowField')}: {fieldLabel(s.pivotTable.rowField)}</span
													>

													{#if fieldHasConditionalHidden(s.pivotTable.rowField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-row-field-conditional-hidden={s.pivotTable.rowField.key}
															>{$t('forms.fieldConditionHiddenBadge')}</span
														>
													{/if}

													{#if fieldHasConditionalReadOnly(s.pivotTable.rowField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-row-field-conditional-readonly={s.pivotTable.rowField.key}
															>{$t('forms.fieldConditionReadOnlyBadge')}</span
														>
													{/if}

													<span>·</span>

													<span data-pivot-column-field={s.pivotTable.columnField.key}
														>{$t('forms.pivotColumnField')}: {fieldLabel(
															s.pivotTable.columnField
														)}</span
													>

													{#if fieldHasConditionalHidden(s.pivotTable.columnField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-column-field-conditional-hidden={s.pivotTable.columnField
																.key}>{$t('forms.fieldConditionHiddenBadge')}</span
														>
													{/if}

													{#if fieldHasConditionalReadOnly(s.pivotTable.columnField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-column-field-conditional-readonly={s.pivotTable.columnField
																.key}>{$t('forms.fieldConditionReadOnlyBadge')}</span
														>
													{/if}

													<span>·</span>
													<span data-pivot-value-field={s.pivotTable.valueField.key}
														>{$t('forms.pivotValueField')}: {fieldLabel(
															s.pivotTable.valueField
														)}</span
													>

													{#if fieldHasConditionalHidden(s.pivotTable.valueField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-value-field-conditional-hidden={s.pivotTable.valueField
																.key}>{$t('forms.fieldConditionHiddenBadge')}</span
														>
													{/if}

													{#if fieldHasConditionalReadOnly(s.pivotTable.valueField)}
														<span
															class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
															data-pivot-value-field-conditional-readonly={s.pivotTable.valueField
																.key}>{$t('forms.fieldConditionReadOnlyBadge')}</span
														>
													{/if}

													<span>·</span>

													<span data-pivot-aggregate={s.pivotTable.aggregate}>
														{$t('forms.pivotAggregate')}: {s.pivotTable.aggregate === 'count'
															? $t('forms.pivotAggregateCount')
															: s.pivotTable.aggregate === 'avg'
																? $t('forms.pivotAggregateAvg')
																: $t('forms.pivotAggregateSum')}
													</span>

													<span>·</span>

													<span data-pivot-totals={s.pivotTable.totalsMode}>
														{$t('forms.pivotTotals')}: {s.pivotTable.totalsMode === 'rows'
															? $t('forms.pivotTotalsRows')
															: s.pivotTable.totalsMode === 'columns'
																? $t('forms.pivotTotalsColumns')
																: s.pivotTable.totalsMode === 'none'
																	? $t('forms.pivotTotalsNone')
																	: $t('forms.pivotTotalsAll')}
													</span>

													<span>·</span>

													<span data-pivot-value-display={s.pivotTable.valueDisplay}>
														{$t('forms.pivotValueDisplay')}: {s.pivotTable.valueDisplay ===
														'percent_row'
															? $t('forms.pivotValueDisplayPercentRow')
															: s.pivotTable.valueDisplay === 'percent_column'
																? $t('forms.pivotValueDisplayPercentColumn')
																: s.pivotTable.valueDisplay === 'percent_total'
																	? $t('forms.pivotValueDisplayPercentTotal')
																	: $t('forms.pivotValueDisplayValue')}
													</span>

													<span>·</span>

													<span data-pivot-sort={s.pivotTable.sortMode}>
														{$t('forms.pivotSort')}: {s.pivotTable.sortMode === 'row_total_desc'
															? $t('forms.pivotSortRowTotalDesc')
															: s.pivotTable.sortMode === 'row_total_asc'
																? $t('forms.pivotSortRowTotalAsc')
																: s.pivotTable.sortMode === 'column_total_desc'
																	? $t('forms.pivotSortColumnTotalDesc')
																	: s.pivotTable.sortMode === 'column_total_asc'
																		? $t('forms.pivotSortColumnTotalAsc')
																		: $t('forms.pivotSortLabel')}
													</span>

													<span>·</span>
													<span data-pivot-empty-buckets={s.pivotTable.emptyBuckets}
														>{$t('forms.pivotEmptyBuckets')}: {s.pivotTable.emptyBuckets
															? $t('forms.yes')
															: $t('forms.no')}</span
													>
													<span>·</span>

													<span data-pivot-hide-zero-buckets={s.pivotTable.hideZeroBuckets}
														>{$t('forms.pivotHideZeroBuckets')}: {s.pivotTable.hideZeroBuckets
															? $t('forms.yes')
															: $t('forms.no')}</span
													>

													<span>·</span>
													<span data-pivot-show-counts={s.pivotTable.showCounts}
														>{$t('forms.pivotShowCounts')}: {s.pivotTable.showCounts
															? $t('forms.yes')
															: $t('forms.no')}</span
													>
													<span>·</span>
													<span data-pivot-heatmap={s.pivotTable.heatmap}
														>{$t('forms.pivotHeatmap')}: {s.pivotTable.heatmap
															? $t('forms.yes')
															: $t('forms.no')}</span
													>
													<span>·</span>

													<span data-pivot-row-limit={s.pivotTable.rowLimit}>
														{$t('forms.pivotRowLimit')}: {s.pivotTable.rowLimit > 0
															? $t('forms.pivotRowLimitValue', {
																	values: { count: s.pivotTable.rowLimit }
																})
															: $t('forms.pivotRowLimitAll')}
													</span>

													<span>·</span>

													<span data-pivot-column-limit={s.pivotTable.columnLimit}>
														{$t('forms.pivotColumnLimit')}: {s.pivotTable.columnLimit > 0
															? $t('forms.pivotColumnLimitValue', {
																	values: { count: s.pivotTable.columnLimit }
																})
															: $t('forms.pivotColumnLimitAll')}
													</span>
												</div>

												<div class="mb-3 flex flex-wrap justify-end gap-2">
													<button
														type="button"
														class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
														onclick={copyPivotTableMatrix}
														data-pivot-copy-table
														data-pivot-table-copied={s.copiedPivotTableMatrix ? 'true' : undefined}
													>
														<Copy class="h-3.5 w-3.5" aria-hidden="true" />
														{$t('forms.copyPivotTable')}
													</button>

													<button
														type="button"
														class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
														onclick={exportPivotTableMatrixCsv}
														data-pivot-export-table
														data-pivot-table-exported={s.exportedPivotTableMatrix
															? 'true'
															: undefined}
													>
														<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
														{$t('forms.exportPivotTable')}
													</button>
												</div>

												<table class="min-w-full divide-y divide-line text-sm">
													<thead
														class="bg-nav text-left text-xs uppercase tracking-wide text-fg-muted"
													>
														<tr>
															<th class="px-4 py-3 font-medium">
																<div class="flex flex-wrap items-center gap-1.5">
																	<span>{fieldLabel(s.pivotTable.rowField)}</span>

																	{#if fieldHasConditionalHidden(s.pivotTable.rowField)}
																		<span
																			class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																			data-pivot-table-row-field-conditional-hidden={s.pivotTable
																				.rowField.key}>{$t('forms.fieldConditionHiddenBadge')}</span
																		>
																	{/if}

																	{#if fieldHasConditionalReadOnly(s.pivotTable.rowField)}
																		<span
																			class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																			data-pivot-table-row-field-conditional-readonly={s.pivotTable
																				.rowField.key}
																			>{$t('forms.fieldConditionReadOnlyBadge')}</span
																		>
																	{/if}
																</div>
															</th>

															{#each s.pivotTable.columns as column}
																<th
																	class="px-4 py-3 text-right font-medium"
																	data-pivot-column={column.label}
																>
																	<div class="flex flex-wrap items-center justify-end gap-1.5">
																		<span>{column.label}</span>

																		{#if recordsFieldHiddenByCondition(s.pivotTable.columnField, pivotColumnRecords(column))}
																			<span
																				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																				data-pivot-column-conditional-hidden={`${column.label}:${s.pivotTable.columnField.key}`}
																				>{$t('forms.fieldConditionHiddenBadge')}</span
																			>
																		{/if}

																		{#if recordsFieldReadOnlyByCondition(s.pivotTable.columnField, pivotColumnRecords(column))}
																			<span
																				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																				data-pivot-column-conditional-readonly={`${column.label}:${s.pivotTable.columnField.key}`}
																				>{$t('forms.fieldConditionReadOnlyBadge')}</span
																			>
																		{/if}
																	</div>
																</th>
															{/each}

															{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
																<th class="px-4 py-3 text-right font-medium"
																	>{$t('forms.pivotTotal')}</th
																>
															{/if}
														</tr>
													</thead>

													<tbody class="divide-y divide-line">
														{#each s.pivotTable.rows as row}
															<tr class="hover:bg-surface" data-pivot-row={row.label}>
																<th class="px-4 py-3 text-left font-medium text-fg">
																	<div class="flex flex-wrap items-center gap-1.5">
																		<span>{row.label}</span>

																		{#if recordsFieldHiddenByCondition(s.pivotTable.rowField, pivotRowRecords(row))}
																			<span
																				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																				data-pivot-row-conditional-hidden={`${row.label}:${s.pivotTable.rowField.key}`}
																				>{$t('forms.fieldConditionHiddenBadge')}</span
																			>
																		{/if}

																		{#if recordsFieldReadOnlyByCondition(s.pivotTable.rowField, pivotRowRecords(row))}
																			<span
																				class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																				data-pivot-row-conditional-readonly={`${row.label}:${s.pivotTable.rowField.key}`}
																				>{$t('forms.fieldConditionReadOnlyBadge')}</span
																			>
																		{/if}
																	</div>
																</th>

																{#each s.pivotTable.columns as column}
																	{@const cellValue = row.cells[column.key] ?? 0}

																	<td
																		class="px-4 py-3 text-right text-fg"
																		data-pivot-cell={`${row.label}:${column.label}`}
																	>
																		<button
																			type="button"
																			class="flex w-full flex-col items-end rounded px-2 py-1 text-right font-medium text-fg hover:bg-surface"
																			style={pivotCellHeatmapStyle(cellValue, s.pivotTable)}
																			onclick={() => selectPivotDrilldown(row, column)}
																			data-pivot-cell-button={`${row.label}:${column.label}`}
																			data-pivot-cell-intensity={pivotCellIntensity(
																				cellValue,
																				s.pivotTable
																			)}
																		>
																			<span
																				>{formatPivotDisplayValue(
																					cellValue,
																					row.total,
																					column.total,
																					s.pivotTable
																				)}</span
																			>

																			{#if s.pivotTable.showCounts}
																				<span
																					class="mt-0.5 text-[11px] font-normal text-fg-muted"
																					data-pivot-cell-count={`${row.label}:${column.label}`}
																					data-record-count={row.cellCounts[column.key] ?? 0}
																					>{$t('forms.pivotCellRecordCount', {
																						values: { count: row.cellCounts[column.key] ?? 0 }
																					})}</span
																				>
																			{/if}

																			{#if recordsFieldHiddenByCondition(s.pivotTable.rowField, pivotCellRecords(row, column))}
																				<span
																					class="mt-0.5 rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																					data-pivot-cell-row-conditional-hidden={`${row.label}:${column.label}:${s.pivotTable.rowField.key}`}
																					>{$t('forms.fieldConditionHiddenBadge')}</span
																				>
																			{/if}

																			{#if recordsFieldReadOnlyByCondition(s.pivotTable.columnField, pivotCellRecords(row, column))}
																				<span
																					class="mt-0.5 rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																					data-pivot-cell-column-conditional-readonly={`${row.label}:${column.label}:${s.pivotTable.columnField.key}`}
																					>{$t('forms.fieldConditionReadOnlyBadge')}</span
																				>
																			{/if}
																		</button>

																		<button
																			type="button"
																			class="mt-1 inline-flex items-center rounded p-1 text-fg-muted hover:bg-surface hover:text-fg"
																			onclick={() => copyPivotCellValue(row, column, cellValue)}
																			aria-label={$t('forms.copyPivotCellValue')}
																			data-pivot-cell-copy={`${row.label}:${column.label}`}
																			data-pivot-cell-copied={s.copiedPivotCellKey ===
																			`${row.label}:${column.label}`
																				? 'true'
																				: undefined}
																			><Copy class="h-3.5 w-3.5" aria-hidden="true" /></button
																		>
																	</td>
																{/each}

																{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
																	<td
																		class="px-4 py-3 text-right font-semibold text-fg"
																		data-pivot-row-total={row.label}
																	>
																		<button
																			type="button"
																			class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
																			onclick={() => selectPivotDrilldown(row, null)}
																			data-pivot-row-total-button={row.label}
																			>{formatPivotTotalDisplayValue(
																				row.total,
																				'row',
																				s.pivotTable
																			)}</button
																		>
																	</td>
																{/if}
															</tr>
														{/each}
													</tbody>

													{#if pivotShowColumnTotals(s.pivotTable.totalsMode)}
														<tfoot class="bg-nav text-sm font-semibold text-fg">
															<tr>
																<th class="px-4 py-3 text-left">{$t('forms.pivotTotal')}</th>

																{#each s.pivotTable.columns as column}
																	<td
																		class="px-4 py-3 text-right"
																		data-pivot-column-total={column.label}
																	>
																		<button
																			type="button"
																			class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
																			onclick={() => selectPivotDrilldown(null, column)}
																			data-pivot-column-total-button={column.label}
																			>{formatPivotTotalDisplayValue(
																				column.total,
																				'column',
																				s.pivotTable
																			)}</button
																		>
																	</td>
																{/each}

																{#if pivotShowRowTotals(s.pivotTable.totalsMode)}
																	<td class="px-4 py-3 text-right" data-pivot-grand-total>
																		<button
																			type="button"
																			class="inline-flex justify-end rounded px-2 py-1 text-right font-semibold text-fg hover:bg-surface"
																			onclick={() => selectPivotDrilldown(null, null)}
																			data-pivot-grand-total-button
																			>{formatPivotTotalDisplayValue(
																				s.pivotTable.grandTotal,
																				'grand',
																				s.pivotTable
																			)}</button
																		>
																	</td>
																{/if}
															</tr>
														</tfoot>
													{/if}
												</table>

												{#if s.pivotDrilldown}
													<section
														class="mt-4 rounded-card border border-line bg-raised p-4"
														data-pivot-drilldown={`${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`}
														data-pivot-drilldown-copied={s.copiedPivotDrilldownKey ===
														pivotDrilldownKey()
															? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
															: undefined}
														data-pivot-drilldown-exported={s.exportedPivotDrilldownKey ===
														pivotDrilldownKey()
															? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
															: undefined}
														data-pivot-drilldown-filters-applied={s.appliedPivotDrilldownFilterKey ===
														pivotDrilldownKey()
															? `${s.pivotDrilldown.rowLabel}:${s.pivotDrilldown.columnLabel}`
															: undefined}
														data-record-count={s.pivotDrilldownRecords.length}
													>
														<div class="mb-3 flex items-start justify-between gap-3">
															<div>
																<h3 class="text-sm font-semibold text-fg">
																	{$t('forms.pivotDrilldown')}
																</h3>

																<p class="mt-1 text-xs text-fg-muted">
																	{$t('forms.pivotDrilldownFor', {
																		values: {
																			row: s.pivotDrilldown.rowLabel,
																			column: s.pivotDrilldown.columnLabel,
																			count: s.pivotDrilldownRecords.length
																		}
																	})}
																</p>
															</div>

															<div class="flex shrink-0 items-center gap-2">
																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	onclick={copyPivotDrilldownRecordIds}
																	disabled={s.pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-copy-ids
																>
																	<Copy class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.copyPivotDrilldownRecordIds')}
																</button>

																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	onclick={exportPivotDrilldownRecordsCsv}
																	disabled={s.pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-export-csv
																	data-pivot-drilldown-exported={s.exportedPivotDrilldownKey ===
																	pivotDrilldownKey()
																		? 'true'
																		: undefined}
																>
																	<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.exportPivotDrilldownRecords')}
																</button>

																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	onclick={applyPivotDrilldownFilters}
																	disabled={!canApplyPivotDrilldownFilters()}
																	data-pivot-drilldown-apply-filters
																	data-pivot-drilldown-filters-applied={s.appliedPivotDrilldownFilterKey ===
																	pivotDrilldownKey()
																		? 'true'
																		: undefined}
																>
																	<Filter class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.applyPivotDrilldownFilters')}
																</button>

																<button
																	type="button"
																	class="inline-flex items-center gap-1 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																	onclick={focusPivotDrilldownRecords}
																	disabled={s.pivotDrilldownRecords.length === 0}
																	data-pivot-drilldown-focus
																>
																	<TableProperties class="h-3.5 w-3.5" aria-hidden="true" />
																	{$t('forms.focusPivotDrilldown')}
																</button>

																<button
																	type="button"
																	class="rounded-control p-1.5 text-fg-muted hover:bg-surface hover:text-fg"
																	onclick={() => (s.pivotDrilldown = null)}
																	aria-label={$t('common.close')}>x</button
																>
															</div>
														</div>

														{#if s.pivotDrilldownRecords.length === 0}
															<p class="text-sm text-fg-muted">
																{$t('forms.pivotDrilldownEmpty')}
															</p>
														{:else}
															<div class="divide-y divide-line">
																{#each s.pivotDrilldownRecords as record}
																	<div
																		class="flex items-center justify-between gap-3 py-2"
																		data-pivot-drilldown-record={record.id}
																	>
																		<div class="min-w-0">
																			<div class="truncate text-sm font-medium text-fg">
																				{record.title}
																			</div>

																			<div class="mt-0.5 text-xs text-fg-muted">
																				{record.id}
																			</div>

																			<div class="mt-1 flex flex-wrap items-center gap-1.5">
																				{#if recordFieldHiddenByCondition(s.pivotTable.rowField, record)}
																					<span
																						class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																						data-pivot-drilldown-row-conditional-hidden={`${record.id}:${s.pivotTable.rowField.key}`}
																						>{$t('forms.fieldConditionHiddenBadge')}</span
																					>
																				{/if}

																				{#if recordFieldReadOnlyByCondition(s.pivotTable.rowField, record)}
																					<span
																						class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																						data-pivot-drilldown-row-conditional-readonly={`${record.id}:${s.pivotTable.rowField.key}`}
																						>{$t('forms.fieldConditionReadOnlyBadge')}</span
																					>
																				{/if}

																				{#if recordFieldHiddenByCondition(s.pivotTable.columnField, record)}
																					<span
																						class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																						data-pivot-drilldown-column-conditional-hidden={`${record.id}:${s.pivotTable.columnField.key}`}
																						>{$t('forms.fieldConditionHiddenBadge')}</span
																					>
																				{/if}

																				{#if recordFieldReadOnlyByCondition(s.pivotTable.columnField, record)}
																					<span
																						class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																						data-pivot-drilldown-column-conditional-readonly={`${record.id}:${s.pivotTable.columnField.key}`}
																						>{$t('forms.fieldConditionReadOnlyBadge')}</span
																					>
																				{/if}
																			</div>
																		</div>

																		<button
																			type="button"
																			class="shrink-0 rounded-control border border-input px-2.5 py-1 text-xs font-medium text-fg hover:bg-surface"
																			onclick={() => selectRecord(record)}
																			data-pivot-drilldown-open={record.id}
																			>{$t('forms.openRecord')}</button
																		>
																	</div>
																{/each}
															</div>
														{/if}
													</section>
												{/if}
											{/if}
										</div>
									{:else if s.selectedViewType === 'detail'}
										<div class="divide-y divide-line" data-view-renderer="detail">
											{#each s.viewRecords as record}
												<article class="p-4 hover:bg-surface">
													<div class="flex items-start justify-between gap-3">
														<button
															type="button"
															class="min-w-0 text-left"
															onclick={() => selectRecord(record)}
															aria-label={`${$t('forms.openRecord')}: ${record.title}`}
														>
															<span class="block truncate font-medium text-fg">{record.title}</span>

															<span class="mt-1 block text-xs text-fg-muted"
																>{$t('forms.updatedAt', {
																	values: { date: formatDate(record.updated_at) }
																})}</span
															>
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

													<div class="mt-3 space-y-3">
														{#each detailLayoutSections() as section}
															<details
																open={!section.collapsible || !section.defaultCollapsed}
																ontoggle={(event) =>
																	keepDetailSectionOpen(event, section.collapsible)}
																data-detail-view-section={`${record.id}:${section.key}`}
																data-detail-view-section-density={`${record.id}:${section.key}:${section.density}`}
																data-detail-view-section-collapsible={`${record.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
																data-detail-view-section-open={`${record.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
															>
																<summary
																	class={section.collapsible
																		? 'cursor-pointer list-none'
																		: 'cursor-default list-none'}
																>
																	<h4
																		class="mb-2 inline text-xs font-semibold uppercase text-fg-muted"
																	>
																		{section.title}
																	</h4>
																</summary>

																{#if section.description}
																	<p
																		class="mb-3 text-sm text-fg-muted"
																		data-detail-view-section-description={`${record.id}:${section.key}`}
																	>
																		{section.description}
																	</p>
																{/if}

																<dl
																	class={detailLayoutSectionGridClass(
																		section.columns,
																		section.density
																	)}
																	data-detail-view-section-columns={`${record.id}:${section.key}:${section.columns}`}
																>
																	{#each detailLayoutSectionFieldsForRecord(section, record) as field}
																		<div
																			class={detailLayoutSectionFieldClass(field, section.columns)}
																			data-detail-view-field={`${record.id}:${section.key}:${field.key}`}
																		>
																			<dt
																				class="truncate text-xs font-medium uppercase text-fg-muted"
																			>
																				{fieldLabel(field)}
																			</dt>

																			<dd class="mt-1 truncate text-sm text-fg">
																				<OptionValueBadges
																					{field}
																					value={record.values[field.key]}
																				/>
																			</dd>
																		</div>
																	{/each}
																</dl>
															</details>
														{/each}
													</div>
												</article>
											{/each}
										</div>
									{:else if s.selectedViewType === 'kanban'}
										<div class="overflow-x-auto p-4" data-view-renderer="kanban">
											<div class="mb-3 text-xs text-fg-muted">
												{$t('forms.viewGroupBy', {
													values: {
														field: s.kanbanGroupField
															? fieldLabel(s.kanbanGroupField)
															: $t('forms.recordTitle')
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

															{#each kanbanMoveOptions(s.kanbanGroupField) as option}
																<option value={option}>{option}</option>
															{/each}
														</select>
													</label>

													<Button
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
												{#each kanbanSwimlaneRows() as swimlane}
													<section
														class={s.kanbanSwimlaneField
															? 'rounded-card border border-line bg-surface p-3'
															: ''}
														data-kanban-swimlane={s.kanbanSwimlaneField
															? swimlane.label
															: undefined}
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
															{#each swimlane.groups as group}
																<section
																	class={`rounded-card border bg-nav transition-colors ${
																		kanbanColumnWipExceeded(group.label)
																			? 'border-line'
																			: 'border-line'
																	}`}
																	data-record-count={group.records.length}
																	data-kanban-column={group.label}
																	data-wip-limited={kanbanColumnWipExceeded(group.label)}
																	role="list"
																	ondragover={(event) => handleKanbanDragOver(event, group.label)}
																	ondrop={(event) => handleKanbanDrop(event, group.label)}
																>
																	<div
																		class="flex items-center justify-between gap-3 border-b border-line px-3 py-2"
																	>
																		<h4 class="truncate text-sm font-semibold text-fg">
																			{group.label}
																		</h4>

																		<span
																			class={`rounded px-2 py-0.5 text-xs ${
																				kanbanColumnWipExceeded(group.label)
																					? 'bg-warning-soft text-warning'
																					: 'bg-raised text-fg-muted'
																			}`}
																			>{kanbanWipLabel(
																				group.label,
																				kanbanColumnRecordCount(group.label)
																			)}</span
																		>
																	</div>

																	<div class="space-y-2 p-2">
																		{#each group.records as record}
																			<article
																				class={`rounded-card border border-line bg-raised p-3 transition-opacity ${s.draggingKanbanRecordId === record.id ? 'opacity-60' : ''}`}
																				draggable={Boolean(s.kanbanGroupField)}
																				data-kanban-card={record.id}
																				role="listitem"
																				ondragstart={(event) =>
																					handleKanbanDragStart(event, record)}
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
																							><span class="block truncate">{record.title}</span
																							></button
																						>
																					</div>

																					<button
																						type="button"
																						class="inline-flex shrink-0 items-center rounded-control p-1 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																						disabled={s.deletingRecordId === record.id}
																						onclick={() => handleDeleteRecord(record)}
																						aria-label={$t('forms.deleteRecord')}
																						><Trash2
																							class="h-3.5 w-3.5"
																							aria-hidden="true"
																						/></button
																					>
																				</div>

																				<div class="mt-2 space-y-1">
																					{#each s.cardFields.slice(0, 3) as field}
																						<p class="truncate text-xs text-fg-muted">
																							<span
																								class="inline-flex items-center gap-1 font-medium"
																							>
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

																							<OptionValueBadges
																								{field}
																								value={record.values[field.key]}
																							/>
																						</p>
																					{/each}
																				</div>

																				{#if s.kanbanGroupField && kanbanMoveOptions(s.kanbanGroupField).length > 0}{@const kanbanField =
																						s.kanbanGroupField}
																					<select
																						value={String(
																							record.values[s.kanbanGroupField.key] ?? ''
																						)}
																						disabled={s.updatingKanbanRecordId === record.id}
																						aria-label={`${$t('forms.kanbanMoveTo')} ${fieldLabel(s.kanbanGroupField)}`}
																						onchange={(event) =>
																							updateKanbanRecordField(
																								record,
																								kanbanField,
																								selectValue(event)
																							)}
																						class="mt-3 w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
																					>
																						{#each kanbanMoveOptions(s.kanbanGroupField) as option}
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
									{:else if s.selectedViewType === 'calendar'}
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
									{:else if s.selectedViewType === 'card'}
										{#snippet cardRecord(record: FormRecord)}
											{@const badgeField = viewCardBadgeField()}
											{@const badge = cardBadge(record)}
											{@const coverUrl = cardCoverUrl(record)}

											<article
												class={s.selectedCardLayout === 'gallery'
													? 'overflow-hidden rounded-card border border-line bg-raised ring-1 ring-transparent transition-shadow hover:shadow-md'
													: s.selectedCardLayout === 'compact' || s.selectedCardLayout === 'list'
														? 'overflow-visible rounded-control border border-line bg-raised'
														: 'overflow-hidden rounded-card border border-line bg-raised'}
												data-card-copied={s.copiedCardRecordId === record.id
													? record.id
													: undefined}
												data-card-exported={s.exportedCardRecordId === record.id
													? record.id
													: undefined}
												data-card-compact-row={s.selectedCardLayout === 'compact'
													? record.id
													: undefined}
												data-card-list-row={s.selectedCardLayout === 'list' ? record.id : undefined}
												data-card-cover-aspect={s.selectedCardCoverAspect}
												data-card-cover-fit={s.selectedCardCoverFit}
												data-card-cover-position={s.selectedCardCoverPosition}
											>
												{#if coverUrl && s.selectedCardLayout !== 'compact' && s.selectedCardLayout !== 'list' && s.selectedCardCoverPosition === 'top'}
													<img
														src={coverUrl}
														alt={cardTitle(record)}
														class={cardCoverImageClass(
															s.selectedCardLayout,
															s.selectedCardCoverAspect,
															s.selectedCardCoverFit
														)}
														data-card-cover={record.id}
														data-card-cover-aspect={s.selectedCardCoverAspect}
														data-card-cover-fit={s.selectedCardCoverFit}
														data-card-cover-position={s.selectedCardCoverPosition}
													/>
												{/if}

												<div
													class={s.selectedCardLayout === 'compact' ||
													s.selectedCardLayout === 'list'
														? 'flex items-center justify-between gap-3 px-3 py-2'
														: s.selectedCardCoverPosition === 'left' && coverUrl
															? 'grid grid-cols-[7rem_minmax(0,1fr)_auto] items-start gap-3 p-4 pb-0'
															: 'flex items-start justify-between gap-3 p-4 pb-0'}
												>
													{#if coverUrl && s.selectedCardLayout !== 'compact' && s.selectedCardLayout !== 'list' && s.selectedCardCoverPosition === 'left'}
														<img
															src={coverUrl}
															alt={cardTitle(record)}
															class={`h-24 w-28 shrink-0 rounded-control ${cardCoverObjectClass(s.selectedCardCoverFit)}`}
															data-card-cover={record.id}
															data-card-cover-aspect={s.selectedCardCoverAspect}
															data-card-cover-fit={s.selectedCardCoverFit}
															data-card-cover-position={s.selectedCardCoverPosition}
														/>
													{/if}

													{#if coverUrl && s.selectedCardLayout === 'compact' && s.selectedCardCoverPosition !== 'hidden'}
														<img
															src={coverUrl}
															alt={cardTitle(record)}
															class={`h-12 w-12 shrink-0 rounded-control ${cardCoverObjectClass(s.selectedCardCoverFit)}`}
															data-card-cover={record.id}
															data-card-cover-aspect={s.selectedCardCoverAspect}
															data-card-cover-fit={s.selectedCardCoverFit}
															data-card-cover-position={s.selectedCardCoverPosition}
														/>
													{/if}

													<button
														type="button"
														class="min-w-0 text-left"
														onclick={() => selectRecord(record)}
														aria-label={`${$t('forms.openRecord')}: ${record.title}`}
													>
														<span
															class={s.selectedCardLayout === 'gallery'
																? 'block truncate text-base font-semibold text-fg'
																: s.selectedCardLayout === 'compact' ||
																	  s.selectedCardLayout === 'list'
																	? 'block truncate text-sm font-semibold text-fg'
																	: 'block truncate font-medium text-fg'}
															data-card-title={record.id}>{cardTitle(record)}</span
														>

														<span
															class="mt-1 block truncate text-xs text-fg-muted"
															data-card-subtitle={record.id}>{cardSubtitle(record)}</span
														>

														{#if s.selectedCardShowTimestamps}
															<span
																class="mt-1 flex flex-wrap gap-x-2 gap-y-0.5 text-[11px] text-fg-muted"
																data-card-timestamps={record.id}
															>
																<span data-card-created-at={record.id}
																	>{$t('forms.cardCreatedAt')}: {formatDate(
																		record.created_at
																	)}</span
																>
																<span data-card-updated-at={record.id}
																	>{$t('forms.cardUpdatedAt')}: {formatDate(
																		record.updated_at
																	)}</span
																>
															</span>
														{/if}

														{#if s.selectedCardShowRecordId}
															<span
																class="mt-1 block truncate font-mono text-[11px] text-fg-muted"
																data-card-record-id={record.id}
																>{$t('forms.cardRecordId')}: {record.id}</span
															>
														{/if}
													</button>

													<div class="flex shrink-0 items-start gap-2">
														{#if badge}
															<span class="max-w-32 truncate" data-card-badge={record.id}
																><OptionValueBadges field={badgeField} value={badge} /></span
															>
														{/if}

														<details class="group relative shrink-0" data-card-menu={record.id}>
															<summary
																class="inline-flex cursor-pointer list-none items-center rounded-control p-1.5 text-fg-muted hover:bg-surface hover:text-fg"
																aria-label={$t('forms.cardActions')}
																><MoreVertical class="h-4 w-4" aria-hidden="true" /></summary
															>

															<div
																class="absolute right-0 z-20 mt-2 w-40 overflow-hidden rounded-float border border-line bg-overlay py-1 text-sm shadow-overlay"
															>
																<button
																	type="button"
																	class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
																	data-card-action={`${record.id}:open`}
																	onclick={() => selectRecord(record)}
																>
																	<FileText class="h-4 w-4" aria-hidden="true" />
																	{$t('forms.openRecord')}
																</button>

																<button
																	type="button"
																	class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
																	data-card-action={`${record.id}:copy-id`}
																	onclick={() => copyRecordId(record)}
																>
																	<Copy class="h-4 w-4" aria-hidden="true" />
																	{$t('forms.copyRecordId')}
																</button>

																<button
																	type="button"
																	class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
																	data-card-action={`${record.id}:export-record`}
																	onclick={() => exportCardRecordDetails(record)}
																>
																	<FileDown class="h-4 w-4" aria-hidden="true" />
																	{$t('forms.exportRecord')}
																</button>

																<button
																	type="button"
																	class="flex w-full items-center gap-2 px-3 py-2 text-left text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																	disabled={s.deletingRecordId === record.id}
																	data-card-action={`${record.id}:delete`}
																	onclick={() => handleDeleteRecord(record)}
																>
																	<Trash2 class="h-4 w-4" aria-hidden="true" />
																	{$t('forms.deleteRecord')}
																</button>
															</div>
														</details>
													</div>
												</div>

												<dl
													class={s.selectedCardLayout === 'compact'
														? 'grid gap-x-4 gap-y-1 border-t border-line px-3 py-2 sm:grid-cols-2 lg:grid-cols-3'
														: s.selectedCardLayout === 'list'
															? 'grid gap-x-4 gap-y-1 border-t border-line px-3 py-2 sm:grid-cols-3 xl:grid-cols-5'
															: 'mt-4 space-y-2 px-4'}
												>
													{#each cardDisplayFields(record) as field}
														<div
															class={s.selectedCardLayout === 'compact' ||
															s.selectedCardLayout === 'list'
																? 'min-w-0 text-xs'
																: 'grid grid-cols-[7rem_minmax(0,1fr)] gap-3 text-sm'}
															data-card-field={`${record.id}:${field.key}`}
														>
															<dt
																class={s.selectedCardLayout === 'compact' ||
																s.selectedCardLayout === 'list'
																	? 'flex min-w-0 flex-wrap items-center gap-1 text-[10px] font-medium uppercase text-fg-muted'
																	: 'flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted'}
															>
																<span class="min-w-0 truncate">{fieldLabel(field)}</span>

																{#if memberFieldHidden(field)}
																	<span
																		class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
																		data-card-field-permission-hidden={`${record.id}:${field.key}`}
																		>{$t('forms.fieldHiddenBadge')}</span
																	>
																{/if}

																{#if memberFieldLocked(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-card-field-permission-locked={`${record.id}:${field.key}`}
																		>{$t('forms.fieldLockedBadge')}</span
																	>
																{/if}

																{#if recordFieldHiddenByCondition(field, record)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-card-field-conditional-hidden={`${record.id}:${field.key}`}
																		>{$t('forms.fieldConditionHiddenBadge')}</span
																	>
																{/if}

																{#if recordFieldReadOnlyByCondition(field, record)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
																		data-card-field-conditional-readonly={`${record.id}:${field.key}`}
																		>{$t('forms.fieldConditionReadOnlyBadge')}</span
																	>
																{/if}
															</dt>

															<dd class="truncate text-fg">
																<OptionValueBadges {field} value={record.values[field.key]} />
															</dd>
														</div>
													{/each}
												</dl>

												{#if s.cardEditableSelectFields.length > 0}
													<div class="mt-4 space-y-2 border-t border-line px-4 py-3">
														{#each s.cardEditableSelectFields as field}
															<label class="block space-y-1">
																<span
																	class="flex flex-wrap items-center gap-1.5 text-xs font-medium text-fg-muted"
																>
																	<span>{fieldLabel(field)}</span>

																	{#if memberFieldHidden(field)}
																		<span
																			class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
																			data-card-select-permission-hidden={`${record.id}:${field.key}`}
																			>{$t('forms.fieldHiddenBadge')}</span
																		>
																	{/if}

																	{#if memberFieldLocked(field)}
																		<span
																			class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																			data-card-select-permission-locked={`${record.id}:${field.key}`}
																			>{$t('forms.fieldLockedBadge')}</span
																		>
																	{/if}

																	{#if recordFieldHiddenByCondition(field, record)}
																		<span
																			class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																			data-card-select-conditional-hidden={`${record.id}:${field.key}`}
																			>{$t('forms.fieldConditionHiddenBadge')}</span
																		>
																	{/if}

																	{#if recordFieldReadOnlyByCondition(field, record)}
																		<span
																			class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																			data-card-select-conditional-readonly={`${record.id}:${field.key}`}
																			>{$t('forms.fieldConditionReadOnlyBadge')}</span
																		>
																	{/if}
																</span>

																<select
																	value={String(record.values[field.key] ?? '')}
																	disabled={s.updatingCardRecordId === record.id}
																	aria-label={`${$t('forms.cardUpdateField')} ${fieldLabel(field)}`}
																	data-card-select={`${record.id}:${field.key}`}
																	onchange={(event) =>
																		updateCardRecordSelectField(record, field, selectValue(event))}
																	class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
																>
																	{#each kanbanMoveOptions(field) as option}
																		<option value={option}>{option}</option>
																	{/each}
																</select>
															</label>
														{/each}
													</div>
												{/if}
											</article>
										{/snippet}

										<div
											class={s.cardGroupField
												? 'space-y-4 p-4'
												: s.selectedCardLayout === 'gallery'
													? 'grid gap-4 p-4 md:grid-cols-2 2xl:grid-cols-4'
													: s.selectedCardLayout === 'compact' || s.selectedCardLayout === 'list'
														? 'space-y-2 p-4'
														: 'grid gap-3 p-4 md:grid-cols-2 xl:grid-cols-3'}
											data-view-renderer="card"
											data-card-layout={s.selectedCardLayout}
											data-card-field-limit={viewCardFieldLimit(
												s.selectedView?.config?.card_field_limit
											)}
											data-card-hide-empty-fields={s.selectedCardHideEmptyFields}
											data-card-show-record-id={s.selectedCardShowRecordId}
											data-card-group-field={s.cardGroupField?.key}
										>
											{#if s.cardGroupField}
												{#each s.cardGroups as group}
													{@const collapsed = cardGroupCollapsed(group)}

													<section
														class="rounded-card border border-line bg-nav p-3"
														data-card-group={group.label}
														data-card-group-collapsed={collapsed ? 'true' : 'false'}
														data-record-count={group.records.length}
														data-card-group-record-ids-copied={s.copiedCardGroupRecordIdsKey ===
														cardGroupFilterKey(group)
															? 'true'
															: undefined}
														data-card-group-records-exported={s.exportedCardGroupRecordsKey ===
														cardGroupFilterKey(group)
															? 'true'
															: undefined}
													>
														<div class="mb-3 flex items-center justify-between gap-3">
															<div class="min-w-0">
																<div class="text-xs font-medium uppercase text-fg-muted">
																	{$t('forms.viewGroupBy', {
																		values: { field: fieldLabel(s.cardGroupField) }
																	})}
																</div>

																<h4 class="truncate text-sm font-semibold text-fg">
																	{group.label}
																</h4>
															</div>

															<div class="flex shrink-0 items-center gap-2">
																<span class="rounded bg-raised px-2 py-0.5 text-xs text-fg-muted"
																	>{$t('forms.groupRecordCount', {
																		values: { count: group.records.length }
																	})}</span
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																	aria-label={$t('forms.copyCardGroupRecordIds')}
																	title={$t('forms.copyCardGroupRecordIds')}
																	disabled={group.records.length === 0}
																	onclick={() => copyCardGroupRecordIds(group)}
																	data-card-group-copy-record-ids={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
																	><Copy class="h-4 w-4" aria-hidden="true" /></button
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																	aria-label={$t('forms.exportCardGroupRecords')}
																	title={$t('forms.exportCardGroupRecords')}
																	disabled={group.records.length === 0}
																	onclick={() => exportCardGroupRecords(group)}
																	data-card-group-export-records={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
																	><FileDown class="h-4 w-4" aria-hidden="true" /></button
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																	aria-label={$t('forms.focusCardGroupRecords')}
																	title={$t('forms.focusCardGroupRecords')}
																	disabled={group.records.length === 0}
																	onclick={() => focusCardGroupRecords(group)}
																	data-card-group-focus-records={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
																	><TableProperties class="h-4 w-4" aria-hidden="true" /></button
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																	aria-label={$t('forms.applyCardGroupFilter')}
																	title={$t('forms.applyCardGroupFilter')}
																	disabled={!canApplyCardGroupFilter(group)}
																	onclick={() => applyCardGroupFilter(group)}
																	data-card-group-apply-filter={`${s.cardGroupField?.key ?? '__none__'}:${group.label}`}
																	data-card-group-filter-applied={s.appliedCardGroupFilterKey ===
																	cardGroupFilterKey(group)
																		? 'true'
																		: undefined}
																	><Filter class="h-4 w-4" aria-hidden="true" /></button
																>

																<button
																	type="button"
																	class="inline-flex h-7 w-7 items-center justify-center rounded-control text-fg-muted hover:bg-raised hover:text-fg"
																	aria-label={collapsed
																		? $t('forms.expandCardGroup')
																		: $t('forms.collapseCardGroup')}
																	onclick={() => toggleCardGroup(group)}
																	data-card-group-toggle={group.label}
																>
																	<ArrowDown
																		class="h-4 w-4 transition-transform {collapsed
																			? '-rotate-90'
																			: ''}"
																		aria-hidden="true"
																	/>
																</button>
															</div>
														</div>

														{#if collapsed}
															<p
																class="rounded-card border border-dashed border-line bg-raised px-3 py-2 text-xs text-fg-muted"
															>
																{$t('forms.cardGroupCollapsed', {
																	values: { count: group.records.length }
																})}
															</p>
														{:else}
															<div
																class={s.selectedCardLayout === 'gallery'
																	? 'grid gap-4 md:grid-cols-2 2xl:grid-cols-4'
																	: s.selectedCardLayout === 'compact' ||
																		  s.selectedCardLayout === 'list'
																		? 'space-y-2'
																		: 'grid gap-3 md:grid-cols-2 xl:grid-cols-3'}
															>
																{#each group.records as record}
																	{@render cardRecord(record)}
																{/each}
															</div>
														{/if}
													</section>
												{/each}
											{:else}
												{#each s.viewRecords as record}
													{@render cardRecord(record)}
												{/each}
											{/if}
										</div>
									{:else}
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
											data-timeline-events-json-exported={s.exportedTimelineEventsJsonCount ??
												undefined}
											data-timeline-event-ids-copied={s.copiedTimelineEventIdsCount ?? undefined}
											data-timeline-aggregate-ids-copied={s.copiedTimelineAggregateIdsCount ??
												undefined}
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
																disabled={timelineEventAggregateKeys(s.timelineEventRows).length ===
																	0}
																onclick={() => copyTimelineAggregateIds(s.timelineEventRows)}
																data-timeline-copy-aggregate-ids
															>
																<Link class="h-3.5 w-3.5" aria-hidden="true" />
																{$t('forms.copyTimelineAggregateIds')}
															</button>

															<button
																type="button"
																class="inline-flex items-center gap-2 rounded-control border border-line px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																disabled={timelineEventLinkedRecordIds(s.timelineEventRows)
																	.length === 0}
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

													{#each s.timelineEventRows as event}
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
																			data-timeline-event-title={event.id}
																			>{timelineEventTitle(event)}</span
																		>

																		<span
																			class="mt-1 block truncate text-xs text-fg-muted"
																			data-timeline-event-subtitle={event.id}
																			>{timelineEventSubtitle(event)}</span
																		>
																	</div>

																	<div class="flex shrink-0 items-center gap-2">
																		<span
																			class="rounded bg-surface px-2 py-0.5 text-xs font-medium text-fg-muted"
																			>{event.event_type}</span
																		>

																		{#if pinned}
																			<span
																				class="rounded bg-warning-soft px-2 py-0.5 text-xs font-medium text-warning"
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
																				><MoreVertical
																					class="h-4 w-4"
																					aria-hidden="true"
																				/></summary
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

																					{pinned
																						? $t('forms.unpinTimelineEvent')
																						: $t('forms.pinTimelineEvent')}
																				</button>

																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
																					data-timeline-event-action={`${event.id}:filter-type`}
																					onclick={() =>
																						applyTimelineEventQuickFilter(event, 'type')}
																				>
																					<FileText class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterType')}
																				</button>

																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface disabled:cursor-not-allowed disabled:bg-surface disabled:text-fg-muted"
																					disabled={!event.actor_id}
																					data-timeline-event-action={`${event.id}:filter-actor`}
																					onclick={() =>
																						applyTimelineEventQuickFilter(event, 'actor')}
																				>
																					<Shield class="h-4 w-4" aria-hidden="true" />
																					{$t('forms.timelineQuickFilterActor')}
																				</button>

																				<button
																					type="button"
																					class="flex w-full items-center gap-2 px-3 py-2 text-left text-fg hover:bg-surface"
																					data-timeline-event-action={`${event.id}:filter-aggregate`}
																					onclick={() =>
																						applyTimelineEventQuickFilter(event, 'aggregate')}
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
																					onclick={() =>
																						copyTimelineEventValue(event, event.id, 'event-id')}
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
																						copyTimelineEventValue(
																							event,
																							timelineEventDetailsJson(event),
																							'details'
																						)}
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

																				<div
																					class="mt-0.5 break-all text-fg"
																					data-timeline-event-aggregate={event.id}
																				>
																					{event.aggregate_type}:{event.aggregate_id}
																				</div>
																			</div>

																			<div>
																				<div class="font-medium text-fg-muted">
																					{$t('forms.timelineEventActor')}
																				</div>

																				<div
																					class="mt-0.5 break-all text-fg"
																					data-timeline-event-actor={event.id}
																				>
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
												{#each s.timelineRecords as record}
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
																	<span
																		class="block truncate font-medium text-fg"
																		data-timeline-title={record.id}>{timelineTitle(record)}</span
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
																		data-timeline-subtitle={record.id}
																		>{timelineSubtitle(record)}</span
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
																	{#each s.timelineEditableSelectFields as field}
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
																					updateTimelineRecordSelectField(
																						record,
																						field,
																						selectValue(event)
																					)}
																				class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg disabled:bg-surface disabled:text-fg-muted"
																			>
																				{#each kanbanMoveOptions(field) as option}
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

<Modal
	bind:open={s.showImportRecords}
	title={$t('forms.importRecords')}
	maxWidthClass="max-w-4xl"
	onclose={closeImportRecords}
>
	<div class="space-y-4" data-import-wizard data-import-wizard-step={s.importWizardStep}>
		<div class="grid gap-2 sm:grid-cols-4" aria-label={$t('forms.importWizardSteps')}>
			{#each ['source', 'mapping', 'preview', 'commit'] as step, index}
				{@const current = s.importWizardStep === step}
				{@const complete =
					(step === 'source' && s.importText.trim().length > 0) ||
					(step === 'mapping' && (s.importMappingApplied || s.importMappingHeaders.length === 0)) ||
					(step === 'preview' && Boolean(s.importPreview)) ||
					(step === 'commit' &&
						(s.importPreview?.valid_rows ?? 0) > 0 &&
						s.importPreview?.invalid_rows === 0)}

				<div
					class={current
						? 'rounded-card border border-line bg-surface p-3 text-fg'
						: complete
							? 'rounded-card border border-line bg-success-soft p-3 text-success'
							: 'rounded-card border border-line bg-raised p-3 text-fg-muted'}
					data-import-step={step}
					data-import-step-active={current ? 'true' : undefined}
				>
					<p class="text-xs font-semibold uppercase tracking-wide">
						{$t('forms.importStepCounter', { values: { step: index + 1 } })}
					</p>

					<p class="mt-1 text-sm font-medium">{$t(`forms.importSteps.${step}.title`)}</p>
					<p class="mt-1 text-xs">{$t(`forms.importSteps.${step}.description`)}</p>
				</div>
			{/each}
		</div>

		{#if s.importWizardStep === 'source'}
			<section class="space-y-3" data-import-wizard-panel="source">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<label class="block text-sm font-medium text-fg" for="form-import-text"
						>{$t('forms.importPayload')}</label
					>

					<div class="flex flex-wrap items-center gap-2">
						<label
							class="inline-flex cursor-pointer items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							for="form-import-file"
						>
							<FileUp class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.uploadImportFile')}
						</label>

						<input
							id="form-import-file"
							class="sr-only"
							type="file"
							accept=".csv,.json,text/csv,application/json"
							onchange={loadImportFile}
							data-import-file-upload
							data-import-file-loaded={s.importedImportFileLoaded ? 'true' : undefined}
							data-import-file-name={s.importedImportFileName}
						/>

						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={exportImportTemplate}
							disabled={!s.selectedForm || s.visibleFields.length === 0}
							data-import-template-export
							data-import-template-exported={s.exportedImportTemplate ? 'true' : undefined}
						>
							<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.downloadImportTemplate')}
						</button>
					</div>
				</div>

				{#if s.importedImportFileName}
					<p class="text-xs text-fg-muted">
						{$t('forms.importFileLoadedName', { values: { name: s.importedImportFileName } })}
					</p>
				{/if}

				<textarea
					id="form-import-text"
					class="min-h-72 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
					value={s.importText}
					oninput={(event) => {
						s.importText = inputValue(event);
						s.importedImportFileName = '';
						s.importedImportFileLoaded = false;
						resetImportPreviewForSourceChange();
						refreshImportMappingFromText();
					}}></textarea>

				<p class="text-xs text-fg-muted">{$t('forms.importPayloadHint')}</p>
			</section>
		{/if}

		{#if s.importWizardStep === 'mapping'}
			<section
				class="space-y-3 rounded-card border border-line bg-nav p-3"
				data-import-wizard-panel="mapping"
				data-import-mapping-wizard
				data-import-mapping-applied={s.importMappingApplied ? 'true' : undefined}
				data-import-mapping-template-available={s.importMappingTemplateAvailable
					? 'true'
					: undefined}
				data-import-mapping-template-saved={s.importMappingTemplateSaved ? 'true' : undefined}
				data-import-mapping-template-restored={s.importMappingTemplateRestored ? 'true' : undefined}
			>
				<div class="flex flex-wrap items-center justify-between gap-2">
					<div>
						<h3 class="text-sm font-medium text-fg">
							{$t('forms.importMapping')}
						</h3>

						<p class="mt-1 text-xs text-fg-muted">
							{$t('forms.importMappingHint')}
						</p>
					</div>

					<div class="flex flex-wrap items-center gap-2">
						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							onclick={saveImportMappingTemplate}
							data-import-mapping-template-save>{$t('forms.saveImportMappingTemplate')}</button
						>

						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={restoreImportMappingTemplate}
							disabled={!s.importMappingTemplateAvailable}
							data-import-mapping-template-restore
							>{$t('forms.restoreImportMappingTemplate')}</button
						>

						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							onclick={applyImportMapping}
							data-import-mapping-apply>{$t('forms.applyImportMapping')}</button
						>
					</div>
				</div>

				<div class="grid max-h-[46vh] gap-2 overflow-auto pr-1 sm:grid-cols-2">
					{#each s.importMappingHeaders as header, index}
						<label class="block space-y-1 text-xs" data-import-mapping-row={index}>
							<span class="font-medium text-fg-muted"
								>{header || $t('forms.importUnnamedColumn')}</span
							>

							<select
								class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
								bind:value={s.importMappingSelections[index]}
								onchange={() => {
									s.importMappingApplied = false;
								}}
								data-import-mapping-column={index}
							>
								<option value="__skip">{$t('forms.importMappingSkip')}</option>
								<option value="__title">{$t('forms.recordTitle')}</option>

								{#each s.fields as field}
									<option value={field.key}>{fieldLabel(field)}</option>
								{/each}
							</select>

							<span class="font-medium text-fg-muted">{$t('forms.importMappingTransform')}</span>

							<select
								class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
								bind:value={s.importMappingTransforms[index]}
								onchange={() => {
									s.importMappingApplied = false;
								}}
								data-import-mapping-transform={index}
							>
								<option value="none">{$t('forms.importMappingTransformNone')}</option>
								<option value="trim">{$t('forms.importMappingTransformTrim')}</option>
								<option value="uppercase">{$t('forms.importMappingTransformUppercase')}</option>
								<option value="lowercase">{$t('forms.importMappingTransformLowercase')}</option>
								<option value="capitalize">{$t('forms.importMappingTransformCapitalize')}</option>
								<option value="titlecase">{$t('forms.importMappingTransformTitlecase')}</option>
								<option value="normalize_spaces"
									>{$t('forms.importMappingTransformNormalizeSpaces')}</option
								>
								<option value="strip_non_digits"
									>{$t('forms.importMappingTransformStripNonDigits')}</option
								>
								<option value="slugify">{$t('forms.importMappingTransformSlugify')}</option>
							</select>
						</label>
					{/each}
				</div>
			</section>
		{/if}

		{#if s.importWizardStep === 'preview'}
			<section class="space-y-3" data-import-wizard-panel="preview">
				{#if s.importPreview}
					<div class="grid gap-2 sm:grid-cols-3">
						<div class="rounded-card border border-line bg-raised p-3">
							<p class="text-xs text-fg-muted">
								{$t('forms.importTotalRows')}
							</p>

							<p class="mt-1 text-2xl font-semibold text-fg">
								{s.importPreview.total_rows}
							</p>
						</div>

						<div class="rounded-card border border-line bg-success-soft p-3">
							<p class="text-xs text-success">
								{$t('forms.importValidRows')}
							</p>

							<p class="mt-1 text-2xl font-semibold text-success">
								{s.importPreview.valid_rows}
							</p>
						</div>

						<div class="rounded-card border border-line bg-danger-soft p-3">
							<p class="text-xs text-danger">{$t('forms.importInvalidRows')}</p>

							<p class="mt-1 text-2xl font-semibold text-danger">
								{s.importPreview.invalid_rows}
							</p>
						</div>
					</div>

					<div class="rounded-control border border-line">
						<div
							class="flex flex-wrap items-center justify-between gap-2 border-b border-line px-3 py-2 text-sm"
						>
							<span class="font-medium text-fg">{$t('forms.importPreview')}</span>

							<span class="text-fg-muted">
								{$t('forms.importPreviewSummary', {
									values: {
										total: s.importPreview.total_rows,
										valid: s.importPreview.valid_rows,
										invalid: s.importPreview.invalid_rows
									}
								})}
							</span>
						</div>

						<div class="max-h-64 overflow-auto">
							<table class="min-w-full divide-y divide-line text-sm">
								<thead class="bg-nav text-left text-xs font-medium uppercase text-fg-muted">
									<tr>
										<th class="px-3 py-2">{$t('forms.importRow')}</th>
										<th class="px-3 py-2">{$t('common.status')}</th>
										<th class="px-3 py-2">{$t('forms.recordTitle')}</th>
										<th class="px-3 py-2">{$t('forms.importMessage')}</th>
									</tr>
								</thead>

								<tbody class="divide-y divide-line">
									{#each s.importPreview.rows as row}
										<tr>
											<td class="px-3 py-2 font-mono text-xs text-fg-muted">{row.row_number}</td>

											<td class="px-3 py-2">
												<span
													class={row.valid
														? 'rounded-full bg-success-soft px-2 py-0.5 text-xs font-medium text-success'
														: 'rounded-full bg-danger-soft px-2 py-0.5 text-xs font-medium text-danger'}
													>{row.valid ? $t('forms.importValid') : $t('forms.importInvalid')}</span
												>
											</td>

											<td class="max-w-56 truncate px-3 py-2 text-fg">{row.title ?? '-'}</td>

											<td class="px-3 py-2 text-fg-muted">{row.error ?? '-'}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					</div>
				{/if}
			</section>
		{/if}

		{#if s.importWizardStep === 'commit'}
			<section
				class="rounded-card border border-line bg-success-soft p-4"
				data-import-wizard-panel="commit"
			>
				<p class="text-sm font-semibold text-success">
					{$t('forms.importReadyToCommit')}
				</p>

				<p class="mt-1 text-sm text-success">
					{#if s.importPreview}
						{$t('forms.importReadyToCommitDescription', {
							values: { count: s.importPreview.valid_rows }
						})}
					{/if}
				</p>
			</section>
		{/if}

		<div
			class="flex flex-col-reverse gap-2 border-t border-line pt-3 sm:flex-row sm:items-center sm:justify-between"
		>
			<div class="flex gap-2">
				{#if s.importWizardStep !== 'source'}
					<Button
						variant="secondary"
						onclick={() => {
							s.importWizardStep =
								s.importWizardStep === 'mapping'
									? 'source'
									: s.importWizardStep === 'preview'
										? 'mapping'
										: 'preview';
						}}
					>
						<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('common.back')}
					</Button>
				{/if}
			</div>

			<div class="flex justify-end gap-2">
				<Button variant="secondary" onclick={closeImportRecords}>{$t('common.cancel')}</Button>

				{#if s.importWizardStep === 'source'}
					<Button
						disabled={!s.importText.trim()}
						loading={s.importPreviewing}
						onclick={continueImportFromSource}
					>
						{$t('forms.importContinueToMapping')}
						<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
					</Button>
				{:else if s.importWizardStep === 'mapping'}
					<Button loading={s.importPreviewing} onclick={continueImportFromMapping}>
						{$t('forms.previewImport')}
						<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
					</Button>
				{:else}
					<Button
						loading={s.importingRecords}
						disabled={!s.importPreview ||
							s.importPreview.invalid_rows > 0 ||
							s.importPreview.valid_rows === 0}
						onclick={commitImportRecords}
					>
						<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.commitImport')}
					</Button>
				{/if}
			</div>
		</div>
	</div>
</Modal>

<Modal
	bind:open={s.designerConfirmOpen}
	title={$t('forms.schemaChangeConfirmTitle')}
	maxWidthClass="max-w-3xl"
	onclose={closeDesignerConfirm}
>
	<div class="space-y-4">
		<div class="rounded-card border border-line bg-warning-soft p-3">
			<div class="flex items-start gap-2 text-sm text-warning">
				<AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
				<p>{$t('forms.schemaChangeConfirmDescription')}</p>
			</div>
		</div>

		<div class="overflow-hidden rounded-control border border-line">
			<table class="min-w-full divide-y divide-line text-sm">
				<thead class="bg-nav text-left text-xs font-medium uppercase text-fg-muted">
					<tr>
						<th class="px-3 py-2">{$t('forms.changeType')}</th>
						<th class="px-3 py-2">{$t('forms.field')}</th>
						<th class="px-3 py-2">{$t('forms.change')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.valueUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.dependencyUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.blockingDependencies')}</th>
					</tr>
				</thead>

				<tbody class="divide-y divide-line bg-raised">
					{#each s.pendingDesignerChanges as change}
						<tr>
							<td class="px-3 py-2 text-fg">
								{#if change.kind === 'delete'}
									{$t('forms.changeDelete')}
								{:else if change.kind === 'rename'}
									{$t('forms.changeRename')}
								{:else}
									{$t('forms.changeTypeChange')}
								{/if}
							</td>

							<td class="px-3 py-2">
								<div class="font-medium text-fg">
									{change.fieldLabel}
								</div>

								<div class="font-mono text-xs text-fg-muted">
									{change.fieldKey}
								</div>
							</td>

							<td class="px-3 py-2 font-mono text-xs text-fg-muted">
								{#if change.to}
									{change.from} -> {change.to}
								{:else}
									{change.from}
								{/if}
							</td>

							<td class="px-3 py-2 text-right font-mono text-fg">{change.valueCount}</td>

							<td class="px-3 py-2 text-right font-mono text-fg">{change.dependencyCount}</td>

							<td
								class="px-3 py-2 text-right font-mono {change.blockingDependencyCount > 0
									? 'text-danger'
									: 'text-fg'}">{change.blockingDependencyCount}</td
							>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={closeDesignerConfirm}>{$t('common.cancel')}</Button>

		<Button variant="dangerSolid" loading={s.designerSaving} onclick={confirmDesignerSchemaSave}
			>{$t('forms.confirmSchemaSave')}</Button
		>
	{/snippet}
</Modal>
