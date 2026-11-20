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

<ImportWizardModal />

<DesignerConfirmModal />
