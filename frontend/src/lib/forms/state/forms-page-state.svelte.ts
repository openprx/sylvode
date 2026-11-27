import { writable, fromStore } from 'svelte/store';
import { FORM_SCHEMA_VERSION } from '#lib/forms/schema-v2.js';
import type { RecordPluginActionSurface } from '#lib/plugins/record-entries.js';
import { type FormsDraftVersion, FormsRefreshLatch } from '#lib/workbench/forms-session.js';
import type {
	BusinessEvent,
	FormAggregate,
	FormAttachment,
	FormChildRecord,
	FormFieldDependency,
	FormFieldUsage,
	FormPermissionAction,
	FormPermissions,
	FormRecord,
	FormRecordComment,
	FormRecordImportPreview,
	FormRecordImportRow,
	FormRecordLink,
	FormRelationTarget,
	FormView,
	UniversalForm,
	FormField
} from '#lib/api/forms.js';
import type { WorkspaceMember } from '#lib/api/members.js';
import type { Project, ScenarioTemplate } from '#lib/api/projects.js';
import type RecordGrid from '#lib/components/forms/RecordGrid.svelte';
import type RecordSidePanel from '#lib/components/forms/RecordSidePanel.svelte';
import {
	PANEL_DEFAULT_WIDTH,
	type PanelStatus
} from '#lib/components/forms/RecordSidePanel.svelte';
import * as precedence from '#lib/forms/view-precedence.js';
import type { FormWorkflowMode } from '#lib/components/forms/FormWorkflowTabs.svelte';
import { page } from '$app/state';
import type { FieldDisplayContext } from '#lib/forms/field-display.js';
import * as arrangement from '#lib/forms/view-arrangement.js';
import { requireRouteParam } from '#lib/utils/route-params.js';
import { recordPageSize } from '#lib/forms/page-constants.js';
import { authStore } from '#lib/stores/auth.js';
import { blankFieldDraft, parseFields } from '#lib/forms/designer-schema.js';
import { detailPageWidgetEntriesFromLayout } from '#lib/forms/detail-layout-drafts.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { currentLocale } from '#lib/forms/i18n.js';
import type {
	CalendarExceptionDateMap,
	CardCoverAspect,
	CardCoverFit,
	CardCoverPosition,
	CardFieldLimit,
	CardLayoutMode,
	ChildEditorState,
	DesignerSchemaChange,
	DetailHeaderDraft,
	DetailHighlightDraft,
	DetailLayoutMode,
	DetailLayoutSectionDraft,
	DetailPageCompositionDraft,
	FieldDraft,
	FormRecordScope,
	GanttScale,
	ImportMappingTransform,
	ImportWizardStep,
	PivotAggregate,
	PivotColumnLimit,
	PivotDrilldownSelection,
	PivotRowLimit,
	PivotSortMode,
	PivotTotalsMode,
	PivotValueDisplay,
	StoredViewArrangement,
	TimelineEventLayout,
	ViewCalendarDayScope,
	ViewCalendarLayout,
	ViewCalendarRange,
	ViewCalendarRecurrence,
	ViewFilterGroupDraft,
	ViewFilterLogic,
	ViewSortDirection,
	ViewTimelineSource,
	ViewVisibility
} from '#lib/forms/page-types.js';
import { defaultPermissionActions } from '#lib/forms/permission-drafts.js';
import { groupRecordsByField, recordsByUpdatedAt } from '#lib/forms/record-grouping.js';
import type { FormsPage } from '#lib/forms/state/forms-page.js';
import {
	builtInView,
	kanbanMoveOptions,
	viewCalendarLayout,
	viewCardCoverAspect,
	viewCardCoverFit,
	viewCardCoverPosition,
	viewCardFieldLimit,
	viewCardLayout,
	viewColumns,
	viewConfigBoolean,
	viewConfigString
} from '#lib/forms/view-config.js';
import {
	eventActors,
	eventAggregates,
	eventsByCreatedAt,
	eventTypes
} from '#lib/forms/views/timeline-events.js';

const authUser = fromStore(authStore);

/** Every piece of reactive state of the forms page (fields, deriveds and the workbench accessor). */
export class FormsPageState {
	readonly fp: FormsPage;

	constructor(fp: FormsPage) {
		this.fp = fp;
	}

	// owner: route
	readonly recordPluginActions: RecordPluginActionSurface = writable(null);

	// owner: loaders
	project = $state<Project | null>(null);
	scenarioTemplates = $state<ScenarioTemplate[]>([]);
	forms = $state<UniversalForm[]>([]);
	formViews = $state<FormView[]>([]);

	// owner: records
	records = $state<FormRecord[]>([]);
	filterPreviewRecords = $state<FormRecord[]>([]);

	// owner: loaders
	formEvents = $state<BusinessEvent[]>([]);
	printJobs = $state<FormRecord[]>([]);

	// owner: attachments
	recordAttachments = $state<FormAttachment[]>([]);
	recordAttachmentSignedUrls = $state<Record<string, string>>({});

	// owner: loaders
	aggregates = $state<Record<string, FormAggregate>>({});
	formPermissions = $state<FormPermissions | null>(null);
	selectedFormId = $state('');
	selectedViewId = $state(precedence.ALL_RECORDS_VIEW_ID);

	// owner: views
	// The working arrangement is compared with the config the draft produced right after the saved
	// view was applied, so representation differences in stored configs never read as changes.
	viewBaseline = $state({
		viewId: '',
		type: 'grid',
		key: '',
		config: {} as Record<string, unknown>
	});
	viewConflict = $state(false);
	saveViewOpen = $state(false);
	saveViewName = $state('');
	saveViewError = $state<string | null>(null);
	resetUndo = $state<StoredViewArrangement | null>(null);

	// owner: loaders
	serverRecordTotal = $state(0);

	// owner: record-panel
	panelRecordId = $state<string | null>(null);
	panelStatus = $state<PanelStatus>('ready');
	panelError = $state('');
	panelDetailsLoading = $state(false);
	panelWidth = $state(PANEL_DEFAULT_WIDTH);

	// owner: markup
	panelAppsOpen = $state(false);

	// owner: record-panel
	panelHistoryDepth = 0;

	// owner: markup
	recordGrid = $state<RecordGrid | null>(null);
	recordPanel = $state<RecordSidePanel | null>(null);

	// owner: record-panel
	liveMessage = $state('');

	// owner: markup
	panelMode = $state<'docked' | 'overlay' | 'sheet'>('docked');

	// owner: records
	selectedRecord = $state<FormRecord | null>(null);
	focusedRecordIds = $state<string[]>([]);

	// owner: loaders
	selectedRecordLinks = $state<FormRecordLink[]>([]);
	selectedRecordChildren = $state<FormChildRecord[]>([]);
	selectedRecordComments = $state<FormRecordComment[]>([]);
	workspaceMembers = $state<WorkspaceMember[]>([]);
	workspaceMembersLoading = $state(false);
	relationTargets = $state<Record<string, FormRelationTarget[]>>({});
	relationTargetsLoading = $state(false);

	// owner: route
	loading = $state(true);

	// owner: loaders
	templatesLoading = $state(false);
	recordsLoading = $state(false);
	eventsLoading = $state(false);

	// owner: record-panel
	recordCommentsLoading = $state(false);

	// owner: create-form
	formSubmitting = $state(false);
	installingTemplateKey = $state<string | null>(null);

	// owner: views
	viewSubmitting = $state(false);

	// owner: exporter
	exportingRecords = $state(false);
	exportedCurrentViewJsonRecordCount = $state<number | null>(null);
	exportedAttachmentManifestCount = $state<number | null>(null);
	exportedAttachmentBundleCount = $state<number | null>(null);
	exportedAttachmentPackageCount = $state<number | null>(null);
	exportedAttachmentPackageFileCount = $state<number | null>(null);

	// owner: permissions
	permissionsLoading = $state(false);
	permissionsSaving = $state(false);

	// owner: importer
	importPreviewing = $state(false);
	importingRecords = $state(false);

	// owner: records
	recordSubmitting = $state(false);

	// owner: attachments
	attachmentSubmittingField = $state<string | null>(null);
	deletingAttachmentId = $state<string | null>(null);

	// owner: record-detail
	linkSubmitting = $state(false);
	recordCommentSubmitting = $state(false);

	// owner: records
	deletingRecordId = $state<string | null>(null);
	restoringRecordId = $state<string | null>(null);
	lastDeletedRecord = $state<{ id: string; title: string } | null>(null);
	recordEditorPanel = $state<HTMLElement | null>(null);

	// owner: view-kanban
	updatingKanbanRecordId = $state<string | null>(null);
	draggingKanbanRecordId = $state<string | null>(null);
	selectedKanbanRecordIds = $state<string[]>([]);

	// owner: markup
	bulkKanbanTarget = $state('');

	// owner: view-kanban
	bulkMovingKanban = $state(false);

	// owner: view-calendar
	updatingCalendarRecordId = $state<string | null>(null);
	draggingCalendarRecordId = $state<string | null>(null);
	appliedCalendarFilterKey = $state<string | null>(null);
	copiedCalendarScheduleKey = $state<string | null>(null);
	shiftedCalendarScheduleKey = $state<string | null>(null);

	// owner: view-cards
	appliedCardGroupFilterKey = $state<string | null>(null);

	// owner: view-gantt
	updatingGanttRecordId = $state<string | null>(null);
	draggingGanttRecordId = $state<string | null>(null);

	// owner: view-cards
	updatingCardRecordId = $state<string | null>(null);

	// owner: session
	collapsedCardGroupKeys = $state<string[]>([]);

	// owner: view-timeline
	updatingTimelineRecordId = $state<string | null>(null);

	// owner: view-cards
	copiedCardRecordId = $state<string | null>(null);
	copiedCardGroupRecordIdsKey = $state<string | null>(null);
	exportedCardGroupRecordsKey = $state<string | null>(null);
	exportedCardRecordId = $state<string | null>(null);

	// owner: view-timeline
	copiedTimelineEventAction = $state<string | null>(null);
	exportedTimelineEventAction = $state<string | null>(null);
	exportedTimelineEventsCsvCount = $state<number | null>(null);
	exportedTimelineEventsJsonCount = $state<number | null>(null);
	copiedTimelineEventIdsCount = $state<number | null>(null);
	copiedTimelineAggregateIdsCount = $state<number | null>(null);

	// owner: session
	pinnedTimelineEventIds = $state<string[]>([]);

	// owner: view-pivot
	copiedPivotDrilldownKey = $state<string | null>(null);
	exportedPivotDrilldownKey = $state<string | null>(null);
	appliedPivotDrilldownFilterKey = $state<string | null>(null);
	copiedPivotCellKey = $state<string | null>(null);
	copiedPivotTableMatrix = $state(false);
	exportedPivotTableMatrix = $state(false);

	// owner: filter-expression-export
	copiedFilterExpressionRecordIds = $state<string | null>(null);
	copiedFilterExpressionJson = $state<string | null>(null);
	copiedFilterExpressionSummary = $state<string | null>(null);
	copiedFilterExpressionCsvRecordIds = $state<string | null>(null);
	copiedFilterExpressionJsonRecordIds = $state<string | null>(null);
	exportedFilterExpressionJson = $state<string | null>(null);
	exportedFilterExpressionSummary = $state<string | null>(null);
	exportedFilterExpressionRecordIds = $state<string | null>(null);
	exportedFilterExpressionJsonRecordIds = $state<string | null>(null);

	// owner: view-pivot
	pivotDrilldown = $state<PivotDrilldownSelection | null>(null);

	// owner: create-form
	showCreateForm = $state(false);

	// owner: importer
	showImportRecords = $state(false);

	// owner: records
	editingRecordId = $state<string | null>(null);
	recordDraft = $state<Record<string, unknown>>({});
	recordTitle = $state('');

	// owner: session
	linkDraft = $state({
		target_type: 'form_record',
		target_id: '',
		relation_key: 'contains',
		relation_type: 'parent_child'
	});

	// owner: record-detail
	recordCommentDraft = $state('');
	childEditor = $state<ChildEditorState | null>(null);
	childSubmitting = $state(false);
	deletingChildRecordId = $state<string | null>(null);

	// owner: create-form
	formDraft = $state({
		key: '',
		name: '',
		description: '',
		title_template: '{id}',
		schemaText: JSON.stringify({ version: FORM_SCHEMA_VERSION, fields: [] }, null, 2)
	});

	// owner: view-draft
	viewDraft = $state({
		key: '',
		name: '',
		view_type: 'grid' as FormView['view_type'],
		columns: [] as string[],
		filterRootLogic: 'all' as ViewFilterLogic,
		filterGroups: [] as ViewFilterGroupDraft[],
		sortField: '',
		sortDirection: 'asc' as ViewSortDirection,
		groupBy: '',
		pivotRowField: '',
		pivotColumnField: '',
		pivotValueField: '',
		pivotAggregate: 'sum' as PivotAggregate,
		pivotTotals: 'all' as PivotTotalsMode,
		pivotValueDisplay: 'value' as PivotValueDisplay,
		pivotSort: 'label' as PivotSortMode,
		pivotEmptyBuckets: false,
		pivotHideZeroBuckets: false,
		pivotShowCounts: false,
		pivotHeatmap: false,
		pivotRowLimit: 0 as PivotRowLimit,
		pivotColumnLimit: 0 as PivotColumnLimit,
		ganttStartField: '',
		ganttEndField: '',
		ganttGroupBy: '',
		ganttDependencyField: '',
		ganttScale: 'day' as GanttScale,
		swimlaneBy: '',
		wipLimits: {} as Record<string, string>,
		dateField: '',
		calendarRange: 'week' as ViewCalendarRange,
		calendarFocusDate: '',
		calendarRecurrence: 'none' as ViewCalendarRecurrence,
		calendarDayScope: 'all' as ViewCalendarDayScope,
		calendarLayout: 'grid' as ViewCalendarLayout,
		calendarHideEmptyDays: false,
		calendarEndField: '',
		calendarResourceBy: '',
		calendarExceptionRecordId: '',
		calendarExceptionDate: '',
		calendarExceptionDates: {} as CalendarExceptionDateMap,
		cardTitleField: '',
		cardSubtitleField: '',
		cardBadgeField: '',
		cardCoverField: '',
		cardLayout: 'standard' as CardLayoutMode,
		cardCoverAspect: 'auto' as CardCoverAspect,
		cardCoverFit: 'cover' as CardCoverFit,
		cardCoverPosition: 'top' as CardCoverPosition,
		cardFieldLimit: 6 as CardFieldLimit,
		cardHideEmptyFields: false,
		cardShowTimestamps: false,
		cardShowRecordId: false,
		cardFieldKeys: [] as string[],
		timelineSource: 'records' as ViewTimelineSource,
		timelineEventLayout: 'summary' as TimelineEventLayout,
		timelineEventType: '',
		timelineEventActor: '',
		timelineEventAggregate: '',
		timelineEventQuery: '',
		timelineEventFrom: '',
		timelineEventTo: '',
		timelineTitleField: '',
		timelineSubtitleField: '',
		visibility: 'shared' as ViewVisibility,
		isDefault: false
	});

	// owner: create-form
	fieldDrafts = $state<FieldDraft[]>([blankFieldDraft()]);

	// owner: records
	activeMode = $state<FormWorkflowMode>('data');

	// owner: session
	detailLayoutMode = $state<DetailLayoutMode>('header');
	viewToolsExpanded = $state(false);

	// owner: records
	recordEditorOpen = $state(false);

	// owner: loaders
	recordPage = $state(1);

	// owner: designer
	designerFieldDrafts = $state<FieldDraft[]>([]);
	designerFieldUsage = $state<FormFieldUsage[]>([]);
	designerFieldDependencies = $state<FormFieldDependency[]>([]);
	designerUsageLoading = $state(false);
	selectedDesignerFieldIndex = $state(0);
	designerSaving = $state(false);

	// owner: session
	detailPageCompositionDraft = $state<DetailPageCompositionDraft>({
		widgetRegion: 'sidebar',
		fieldColumns: 2
	});

	// owner: detail-layout-editor
	detailPageCompositionSaving = $state(false);
	detailPageCompositionSyncedFingerprint = $state('');
	detailPageWidgetSaving = $state(false);
	detailLayoutSectionDrafts = $state<DetailLayoutSectionDraft[]>([]);
	detailLayoutSectionSaving = $state(false);
	detailLayoutSectionSyncedFingerprint = $state('');
	detailHeaderDraft = $state<DetailHeaderDraft>({ subtitleField: '', badgeField: '' });
	detailHeaderSaving = $state(false);
	detailHeaderSyncedFingerprint = $state('');
	detailHighlightDraft = $state<DetailHighlightDraft>({ fields: [] });
	detailHighlightSaving = $state(false);
	detailHighlightSyncedFingerprint = $state('');

	// owner: designer
	designerSyncedFormId = $state('');
	designerConfirmOpen = $state(false);
	pendingDesignerSchema = $state<Record<string, unknown> | null>(null);
	pendingDesignerChanges = $state<DesignerSchemaChange[]>([]);

	// owner: create-form
	showAdvancedSchema = $state(false);

	// owner: importer
	importText = $state('');
	importRows = $state<FormRecordImportRow[]>([]);
	importPreview = $state<FormRecordImportPreview | null>(null);
	exportedImportTemplate = $state(false);
	importedImportFileName = $state('');
	importedImportFileLoaded = $state(false);
	importMappingHeaders = $state<string[]>([]);
	importMappingSelections = $state<string[]>([]);
	importMappingTransforms = $state<ImportMappingTransform[]>([]);
	importMappingApplied = $state(false);
	importMappingTemplateAvailable = $state(false);
	importMappingTemplateSaved = $state(false);
	importMappingTemplateRestored = $state(false);
	importWizardStep = $state<ImportWizardStep>('source');

	// owner: permissions
	memberPermissionDraft = $state<Record<FormPermissionAction, boolean>>(defaultPermissionActions());
	memberRecordScopeDraft = $state<FormRecordScope>('all');
	memberFieldReadDraft = $state<Record<string, boolean>>({});
	memberFieldWriteDraft = $state<Record<string, boolean>>({});

	// owner: session
	formsSessionKey = $state<string | null>(null);
	formsSessionUser: string | null = null;
	formsSessionRelease: (() => void) | null = null;

	// owner: route
	formsSessionReady = $state(false);

	// owner: session
	formsSessionRemoteChanged = $state(false);
	formsSessionRefreshFailed = $state(false);
	formsSessionConflict = $state(false);
	formsSessionReview = $state(false);
	formsSessionClean = $state<Record<string, string>>({});

	// owner: records
	formsSessionRecordVersion = $state<FormsDraftVersion | null>(null);

	// owner: session
	formsSessionSchemaVersion = $state<number | null>(null);
	formsSessionViewVersion = $state<string | null>(null);
	formsSessionGeneration = 0;
	formsSessionActivating = '';
	formsSessionOpening: Promise<void> | null = null;
	formsSessionOpenRequest = 0;
	formsSessionTransitioning = $state(false);

	// owner: markup
	formsSessionRoot = $state<HTMLElement | null>(null);

	// owner: session
	formsSessionFocus: { selector: string; start: number | null; end: number | null } | null = null;
	readonly formsSessionLatch = new FormsRefreshLatch();
	formsSessionRemoteRecord = $state<FormRecord | null>(null);
	formsSessionRemoteSchema = $state<number | null>(null);

	// owner: route
	arrangementTimer: ReturnType<typeof setTimeout> | undefined;

	// owner: loaders
	loadedRecordQuery = '';

	// owner: route
	recordQueryTimer: ReturnType<typeof setTimeout> | undefined;

	// owner: record-panel
	announceTimer: ReturnType<typeof setTimeout> | undefined;
	// URL writes run one at a time: a replace issued while a push is still in flight would abort
	// it, and the panel's history depth (what Close and Back undo) would no longer be true.
	recordUrlWrite: Promise<void> = Promise.resolve();
	// Closing can navigate (history.back); focus the row again once that navigation settles.
	pendingRowFocus: string | null = null;

	// owner: attachments
	activeSignatureCanvas: HTMLCanvasElement | null = null;

	// deriveds
	readonly workspaceId = $derived(requireRouteParam(page.params.workspaceId, 'workspaceId'));

	readonly projectId = $derived(requireRouteParam(page.params.projectId, 'projectId'));

	readonly selectedForm = $derived.by(
		() => this.forms.find((form) => form.id === this.selectedFormId) ?? null
	);

	readonly printJobForm = $derived.by(
		() => this.forms.find((form) => form.key === 'print_job') ?? null
	);

	readonly fields = $derived(this.selectedForm ? parseFields(this.selectedForm.schema) : []);

	readonly detailVisibleFields = $derived(
		this.fields.filter((field) => field.visibility?.detail !== false)
	);

	readonly savedView = $derived(
		this.formViews.find((view) => view.id === this.selectedViewId) ?? null
	);

	readonly baseView = $derived(
		this.savedView ?? (this.selectedForm ? builtInView(this.selectedForm) : null)
	);

	readonly draftViewConfig = $derived.by(() => this.fp.viewDraft.viewConfigFromDraft());

	readonly arrangementModified = $derived(
		!!this.baseView &&
			this.viewBaseline.viewId === this.baseView.id &&
			(this.viewBaseline.type !== this.viewDraft.view_type ||
				this.viewBaseline.key !== arrangement.normalizedViewConfig(this.draftViewConfig))
	);

	readonly selectedView = $derived<FormView | null>(
		this.baseView && this.arrangementModified
			? { ...this.baseView, view_type: this.viewDraft.view_type, config: this.draftViewConfig }
			: this.baseView
	);

	readonly recordQuery = $derived.by(() =>
		arrangement.serverQueryFor(
			this.savedView && { id: this.savedView.id, config: this.viewBaseline.config },
			this.draftViewConfig,
			{ modified: this.arrangementModified, isServerSortable: this.fp.views.isServerSortable }
		)
	);

	readonly canDesign = $derived(this.formPermissions?.effective.actions['form.design'] ?? true);

	readonly selectedViewColumns = $derived.by(() => viewColumns(this.selectedView));

	readonly selectedViewType = $derived(
		(this.selectedView?.view_type ?? 'grid') as FormView['view_type']
	);

	readonly detailUrlRecordId = $derived(
		page.url.searchParams.get('record') ?? page.url.searchParams.get('record_id') ?? ''
	);

	readonly focusedRecords = $derived.by(() => {
		if (this.focusedRecordIds.length === 0) return [];

		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- plain lookup rebuilt on each derivation, not reactive state
		const focusedIds = new Set(this.focusedRecordIds);

		return this.records.filter((record) => focusedIds.has(record.id));
	});

	readonly memberHiddenFieldLabels = $derived(
		this.fields
			.filter((field) => this.memberFieldReadDraft[field.key] === false)
			.map((field) => fieldLabel(field))
	);

	readonly memberHiddenFieldKeys = $derived(
		this.fields
			.filter((field) => this.memberFieldReadDraft[field.key] === false)
			.map((field) => field.key)
	);

	readonly memberLockedFieldLabels = $derived(
		this.fields
			.filter((field) => this.memberFieldWriteDraft[field.key] === false)
			.map((field) => fieldLabel(field))
	);

	readonly memberLockedFieldKeys = $derived(
		this.fields
			.filter((field) => this.memberFieldWriteDraft[field.key] === false)
			.map((field) => field.key)
	);

	readonly listableFields = $derived(
		this.fields.filter((field) => field.visibility?.list !== false)
	);

	readonly visibleFields = $derived.by(() => {
		if (this.selectedViewColumns.length === 0) return this.listableFields;

		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- plain lookup rebuilt on each derivation, not reactive state
		const byKey = new Map(this.listableFields.map((field) => [field.key, field]));
		const ordered = this.selectedViewColumns
			.map((key) => byKey.get(key))
			.filter((field): field is FormField => Boolean(field));

		return ordered.length > 0 ? ordered : this.listableFields;
	});

	readonly cardFields = $derived.by(() =>
		this.fp.viewCards
			.orderedCardFieldsForView(this.selectedView)
			.slice(0, viewCardFieldLimit(this.selectedView?.config?.card_field_limit))
	);

	readonly selectedCardLayout = $derived(viewCardLayout(this.selectedView?.config?.card_layout));

	readonly selectedCardCoverAspect = $derived(
		viewCardCoverAspect(this.selectedView?.config?.card_cover_aspect)
	);

	readonly selectedCardCoverFit = $derived(
		viewCardCoverFit(this.selectedView?.config?.card_cover_fit)
	);

	readonly selectedCardCoverPosition = $derived(
		viewCardCoverPosition(this.selectedView?.config?.card_cover_position)
	);

	readonly selectedCardHideEmptyFields = $derived(
		viewConfigBoolean(this.selectedView, 'card_hide_empty_fields')
	);

	readonly selectedCardShowTimestamps = $derived(
		viewConfigBoolean(this.selectedView, 'card_show_timestamps')
	);

	readonly selectedCardShowRecordId = $derived(
		viewConfigBoolean(this.selectedView, 'card_show_record_id')
	);

	readonly selectedCalendarLayout = $derived(
		viewCalendarLayout(this.selectedView?.config?.calendar_layout)
	);

	readonly selectedCalendarHideEmptyDays = $derived(
		viewConfigBoolean(this.selectedView, 'calendar_hide_empty_days')
	);

	readonly cardGroupField = $derived.by(() =>
		this.selectedViewType === 'card'
			? this.fp.viewRecords.fieldByKey(viewConfigString(this.selectedView, 'group_by'))
			: null
	);

	readonly cardGroups = $derived.by(() =>
		this.cardGroupField ? groupRecordsByField(this.viewRecords, this.cardGroupField) : []
	);

	readonly cardEditableSelectFields = $derived(
		this.cardFields.filter(
			(field) => field.type === 'single_select' && kanbanMoveOptions(field).length > 0
		)
	);

	readonly timelineEditableSelectFields = $derived(this.cardEditableSelectFields);

	readonly viewRecords = $derived.by(() => this.fp.viewRecords.applyViewRecordConfig(this.records));

	readonly recordPageCount = $derived(
		Math.max(1, Math.ceil(this.viewRecords.length / recordPageSize))
	);

	readonly normalizedRecordPage = $derived(
		Math.min(Math.max(this.recordPage, 1), this.recordPageCount)
	);

	readonly paginatedViewRecords = $derived.by(() => {
		const start = (this.normalizedRecordPage - 1) * recordPageSize;

		return this.viewRecords.slice(start, start + recordPageSize);
	});

	readonly kanbanGroupField = $derived.by(() => this.fp.viewKanban.viewGroupField());

	readonly kanbanGroups = $derived.by(() =>
		groupRecordsByField(this.viewRecords, this.kanbanGroupField)
	);

	readonly kanbanSwimlaneField = $derived.by(() => this.fp.viewKanban.viewSwimlaneField());

	readonly kanbanSwimlanes = $derived.by(() =>
		this.fp.viewKanban.groupKanbanSwimlanes(this.viewRecords)
	);

	readonly pivotTable = $derived.by(() => this.fp.viewPivot.buildPivotTable(this.viewRecords));

	readonly pivotDrilldownRecords = $derived.by(() =>
		this.fp.viewPivot.buildPivotDrilldownRecords(this.viewRecords)
	);

	readonly ganttTimeline = $derived.by(() =>
		this.fp.viewGantt.buildGanttTimeline(this.viewRecords)
	);

	readonly calendarDateField = $derived.by(() => this.fp.viewCalendar.viewDateField());

	readonly calendarEndField = $derived.by(() => this.fp.viewCalendar.viewCalendarEndField());

	readonly calendarGridGroups = $derived.by(() =>
		this.fp.viewCalendar.groupRecordsByCalendarGrid(this.viewRecords, this.calendarDateField)
	);

	readonly calendarResourceField = $derived.by(() =>
		this.fp.viewCalendar.viewCalendarResourceField()
	);

	readonly calendarResourceLanes = $derived.by(() =>
		this.fp.viewCalendar.groupCalendarResourceLanes(this.viewRecords)
	);

	readonly timelineRecords = $derived.by(() => recordsByUpdatedAt(this.viewRecords));

	readonly timelineEventTypes = $derived.by(() => eventTypes(this.formEvents));

	readonly timelineEventActors = $derived.by(() => eventActors(this.formEvents));

	readonly timelineEventAggregates = $derived.by(() => eventAggregates(this.formEvents));

	readonly timelineEvents = $derived.by(() =>
		eventsByCreatedAt(this.fp.viewTimeline.filteredTimelineEvents(this.formEvents))
	);

	readonly timelineEventRows = $derived.by(() =>
		this.fp.viewTimeline.orderedTimelineEvents(this.timelineEvents)
	);

	readonly selectedRecordEvents = $derived.by(() =>
		this.selectedRecord
			? eventsByCreatedAt(
					this.formEvents.filter(
						(event) =>
							event.aggregate_type === 'form_record' &&
							event.aggregate_id === this.selectedRecord?.id
					)
				)
			: []
	);

	readonly timelineTitlePermissionField = $derived.by(() =>
		this.fp.viewTimeline.viewTimelineTitleField()
	);

	readonly timelineSubtitlePermissionField = $derived.by(() =>
		this.fp.viewTimeline.viewTimelineSubtitleField()
	);

	readonly selectedDesignerField = $derived(
		this.designerFieldDrafts[this.selectedDesignerFieldIndex] ?? null
	);

	readonly selectedDesignerFieldUsage = $derived(
		this.selectedDesignerField
			? (this.designerFieldUsage.find(
					(usage) =>
						(this.selectedDesignerField.field_id &&
							usage.field_id === this.selectedDesignerField.field_id) ||
						usage.field_key === this.selectedDesignerField.key
				) ?? null)
			: null
	);

	readonly selectedDesignerFieldDependencies = $derived(
		this.selectedDesignerField
			? this.designerFieldDependencies.filter(
					(dependency) =>
						(this.selectedDesignerField.field_id &&
							dependency.field_id === this.selectedDesignerField.field_id) ||
						dependency.field_key === this.selectedDesignerField.key
				)
			: []
	);

	readonly selectedDetailPageWidgetEntries = $derived.by(() =>
		detailPageWidgetEntriesFromLayout(this.selectedForm?.detail_layout)
	);

	readonly currentUserId = $derived(
		authUser.current.user?.id ?? this.workbench?.current.userId ?? ''
	);

	// --- record side panel ---------------------------------------------------------------
	readonly panelRecord = $derived(
		this.panelRecordId
			? (this.records.find((record) => record.id === this.panelRecordId) ??
					(this.selectedRecord?.id === this.panelRecordId ? this.selectedRecord : null))
			: null
	);

	readonly panelPosition = $derived(
		this.panelRecordId
			? this.viewRecords.findIndex((record) => record.id === this.panelRecordId)
			: -1
	);

	readonly fieldCtx = $derived<FieldDisplayContext>({
		locale: currentLocale() ?? 'en',
		formKey: this.selectedForm?.key ?? '',
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- plain lookup rebuilt on each derivation, not reactive state
		members: new Map(this.workspaceMembers.map((member) => [member.user_id, member])),
		attachmentsByField: this.recordAttachments.reduce<Record<string, FormAttachment[]>>(
			(groups, item) => {
				if (item.record_id === this.panelRecordId) (groups[item.field_key] ??= []).push(item);
				return groups;
			},
			{}
		),
		signedUrls: this.recordAttachmentSignedUrls,
		relationTitle: (field, id) =>
			this.relationTargets[field.key]?.find((item) => item.record_id === id)?.display
	});

	get workbench() {
		return this.fp.deps.workbench;
	}

	/** `$state.snapshot` for the controllers, which are plain .ts files and cannot use runes. */
	readonly snapshot = <T>(value: T): ReturnType<typeof $state.snapshot<T>> =>
		$state.snapshot(value);
}
