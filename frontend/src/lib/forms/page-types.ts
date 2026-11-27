import type { FormField, FormFieldType, FormRecord } from '#lib/api/forms.js';
import type { DefaultMode, RuleDraft } from '#lib/forms/rule-drafts.js';
import type * as arrangement from '#lib/forms/view-arrangement.js';

export type ImportMappingTransform =
	| 'none'
	| 'trim'
	| 'uppercase'
	| 'lowercase'
	| 'capitalize'
	| 'titlecase'
	| 'normalize_spaces'
	| 'strip_non_digits'
	| 'slugify';

export type ImportWizardStep = 'source' | 'mapping' | 'preview' | 'commit';

export type AttachmentBundleEntry = {
	record_id: string;
	record_title: string;
	field_key: string;
	field_label: string;
	field_type: string;
	url: string;
	download_url: string;
	thumbnail_url: string | null;
	media_metadata?: Record<string, unknown> | null;
	file_name: string;
	content_type: string;
	byte_size: number;
	storage_key: string | null;
	attachment_id: string | null;
	source: string;
};

export type AttachmentBundleExport = {
	form_id: string;
	form_key: string;
	view_id: string | null;
	view_name: string | null;
	exported_at: string;
	record_count: number;
	attachment_count: number;
	bundle_format: string;
	attachments: AttachmentBundleEntry[];
};

export type ZipFileInput = { path: string; data: string | Uint8Array };

export type StoredViewArrangement = arrangement.StoredArrangement;

export type FieldDraft = {
	field_id?: string;
	key: string;
	label: string;
	type: FormFieldType;
	required: boolean;
	optionsText: string;
	optionColorsText: string;
	optionDisabledText: string;
	optionArchivedText: string;
	optionDescriptionsText: string;
	optionGroupsText: string;
	currency: string;
	scale: string;
	placeholder: string;
	helpText: string;
	defaultMode: DefaultMode;
	defaultValue: string;
	defaultCel: string;
	/** `rules.validate`, one draft per rule. */
	rules: RuleDraft[];
	/** `rules.normalize` as stored; the designer toggles built-in ops and keeps plugin refs. */
	normalize: unknown[];
	/** `rules` members other than `validate` and `normalize` (`transform`, unknown), as stored. */
	rulesExtra: Record<string, unknown>;
	/** Field members the designer does not edit (`mask`, `signature`, unknown keys), as stored. */
	extra: Record<string, unknown>;
	listVisible: boolean;
	detailVisible: boolean;
	conditionalHiddenWhenText: string;
	conditionalReadOnlyWhenText: string;
	readOnly: boolean;
	formulaOp: string;
	formulaArgsText: string;
	formulaScale: string;
	formulaRelationKey: string;
	formulaChildField: string;
	relationTargetType: string;
	relationFormKey: string;
	relationKey: string;
	relationType: string;
	relationDisplayField: string;
	attachmentAcceptText: string;
	attachmentMaxSizeMb: string;
	attachmentMultiple: boolean;
	attachmentPreview: boolean;
	attachmentThumbnailFormat: string;
	attachmentPreviewFormat: string;
	attachmentVariantsText: string;
	attachmentStoragePolicy: string;
	attachmentRetentionDays: string;
	attachmentSignedUrlTtlMinutes: string;
};

export type DesignerSchemaChange = {
	kind: 'delete' | 'rename' | 'type_change';
	field_id?: string;
	fieldKey: string;
	fieldLabel: string;
	from?: string;
	to?: string;
	valueCount: number;
	indexedCount: number;
	dependencyCount: number;
	blockingDependencyCount: number;
};

export type DetailPageWidgetKey = 'children' | 'links' | 'comments' | 'attachments' | 'events';

export type DetailPageWidgetRegion = 'sidebar' | 'below';

export type DetailPageWidgetDensity = 'comfortable' | 'compact';

export type DetailPageWidgetEntry = {
	key: DetailPageWidgetKey;
	enabled: boolean;
	title: string;
	description: string;
	emptyText: string;
	showCount: boolean;
	hideEmpty: boolean;
	collapsible: boolean;
	defaultCollapsed: boolean;
	density: DetailPageWidgetDensity;
	limit: string;
};

export type DetailPageCompositionDraft = {
	widgetRegion: DetailPageWidgetRegion;
	fieldColumns: 1 | 2;
};

export type ChildEditorState = {
	relationFieldKey: string;
	childFormId: string;
	recordId?: string;
	title: string;
	draft: Record<string, unknown>;
};

export type ViewFilterOperator = 'contains' | 'equals' | 'not_equals' | 'not_empty';

export type ViewSortDirection = 'asc' | 'desc';

export type ViewVisibility = 'shared' | 'private';

export type ViewFilterLogic = 'all' | 'any';

export type ViewCalendarRange = 'week' | 'month';

export type ViewCalendarRecurrence = 'none' | 'daily' | 'weekly' | 'monthly';

export type ViewCalendarDayScope = 'all' | 'workweek';

export type ViewCalendarLayout = 'grid' | 'agenda';

export type CardLayoutMode = 'standard' | 'gallery' | 'compact' | 'list';

export type CardFieldLimit = 3 | 6 | 9 | 99;

export type CardCoverAspect = 'auto' | 'wide' | 'square' | 'portrait';

export type CardCoverFit = 'cover' | 'contain';

export type CardCoverPosition = 'top' | 'left' | 'hidden';

export type PivotAggregate = 'sum' | 'count' | 'avg';

export type PivotTotalsMode = 'all' | 'rows' | 'columns' | 'none';

export type PivotValueDisplay = 'value' | 'percent_row' | 'percent_column' | 'percent_total';

export type PivotSortMode =
	| 'label'
	| 'row_total_desc'
	| 'row_total_asc'
	| 'column_total_desc'
	| 'column_total_asc';

export type PivotRowLimit = 0 | 2 | 5 | 10;

export type PivotColumnLimit = 0 | 2 | 5 | 10;

export type CalendarExceptionDateMap = Record<string, string[]>;

export type FormRecordScope = 'all' | 'owned';

export type DetailLayoutSectionDensity = 'comfortable' | 'compact';

export type DetailLayoutSection = {
	key: string;
	title: string;
	description: string;
	columns: 1 | 2 | 3;
	density: DetailLayoutSectionDensity;
	hideEmptyFields: boolean;
	collapsible: boolean;
	defaultCollapsed: boolean;
	fields: FormField[];
};

export type DetailLayoutSectionDraft = {
	key: string;
	title: string;
	description: string;
	columns: 1 | 2 | 3;
	density: DetailLayoutSectionDensity;
	hideEmptyFields: boolean;
	collapsible: boolean;
	defaultCollapsed: boolean;
	fields: string[];
};

export type DetailHeaderDraft = { subtitleField: string; badgeField: string };

export type DetailHighlightDraft = { fields: string[] };

export type ViewFilterDraft = {
	field: string;
	operator: ViewFilterOperator;
	value: string;
	disabled?: boolean;
};

export type ViewFilterGroupDraft = {
	logic: ViewFilterLogic;
	filters: ViewFilterDraft[];
	disabled?: boolean;
};

export type ParsedViewFilterExpression =
	| {
			kind: 'condition';
			filter: ParsedViewFilter;
			disabled?: boolean;
	  }
	| {
			kind: 'group';
			logic: ViewFilterLogic;
			children: ParsedViewFilterExpression[];
			disabled?: boolean;
	  };

export type CalendarRecordOccurrence = {
	key: string;
	record: FormRecord;
	date: string;
	startDate: string;
	endDate: string;
	recurring: boolean;
	spanStart: boolean;
	spanEnd: boolean;
	spanDays: number;
};

export type CalendarGridGroup = {
	key: string;
	label: string;
	date: string;
	records: CalendarRecordOccurrence[];
};

export type CalendarResourceRow = {
	key: string;
	label: string;
	records: FormRecord[];
	groups: CalendarGridGroup[];
};

export type GanttTask = {
	record: FormRecord;
	start: string;
	end: string;
	dependencyValue: string;
	startOffset: number;
	span: number;
};

export type GanttScale = 'day' | 'week' | 'month';

export type GanttTimeline = {
	startField: FormField | null;
	endField: FormField | null;
	groupField: FormField | null;
	dependencyField: FormField | null;
	scale: GanttScale;
	dates: string[];
	lanes: Array<{ key: string; label: string; tasks: GanttTask[] }>;
};

export type PivotTable = {
	rowField: FormField | null;
	columnField: FormField | null;
	valueField: FormField | null;
	valueCurrency: string;
	aggregate: PivotAggregate;
	totalsMode: PivotTotalsMode;
	valueDisplay: PivotValueDisplay;
	sortMode: PivotSortMode;
	emptyBuckets: boolean;
	hideZeroBuckets: boolean;
	showCounts: boolean;
	heatmap: boolean;
	rowLimit: PivotRowLimit;
	columnLimit: PivotColumnLimit;
	maxCellValue: number;
	columns: Array<{ key: string; label: string; total: number }>;
	rows: Array<{
		key: string;
		label: string;
		total: number;
		cells: Record<string, number>;
		cellCounts: Record<string, number>;
	}>;
	grandTotal: number;
};

export type PivotDrilldownSelection = {
	rowKey: string | null;
	rowLabel: string;
	columnKey: string | null;
	columnLabel: string;
};

export type ViewTimelineSource = 'records' | 'events';

export type TimelineEventLayout = 'summary' | 'detailed';

export type DetailLayoutMode = 'header' | 'highlights' | 'composition' | 'sections' | 'widgets';

export type ParsedViewFilter = {
	field: FormField;
	operator: ViewFilterOperator;
	value: string;
};

export type ParsedViewFilterGroup = { logic: ViewFilterLogic; filters: ParsedViewFilter[] };

export type FieldConditionOperator =
	| 'equals'
	| 'not_equals'
	| 'contains'
	| 'not_contains'
	| 'is_empty'
	| 'not_empty'
	| 'gt'
	| 'gte'
	| 'lt'
	| 'lte';

export type FieldConditionItem = {
	field: string;
	equals?: string;
	operator?: FieldConditionOperator;
	value?: string;
};

export type FieldConditionDraft =
	| FieldConditionItem
	| { logic: 'all' | 'any'; conditions: FieldConditionItem[] };

export type SignatureDraftScope = 'record' | 'child';
