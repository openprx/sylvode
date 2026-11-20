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

	// forms-split slot W1-13

	// forms-split slot W1-14

	// forms-split slot W1-15

	// forms-split slot W1-16

	// forms-split slot W1-17

	import RecordDetailAside from '#lib/components/forms/page/RecordDetailAside.svelte';

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
