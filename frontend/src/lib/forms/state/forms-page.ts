import type { taskEditorAction } from '#lib/workbench/edit-surface.js';
import type { TaskWorkbench } from '#lib/workbench/tabs.js';
import { FormsPageState } from '#lib/forms/state/forms-page-state.svelte.js';
import { SessionController } from '#lib/forms/state/session.js';
import { LoadersController } from '#lib/forms/state/loaders.js';
import { RecordsController } from '#lib/forms/state/records.js';
import { RecordPanelController } from '#lib/forms/state/record-panel.js';
import { RecordDetailController } from '#lib/forms/state/record-detail.js';
import { AttachmentsController } from '#lib/forms/state/attachments.js';
import { CreateFormController } from '#lib/forms/state/create-form.js';
import { DesignerController } from '#lib/forms/state/designer.js';
import { DetailLayoutEditorController } from '#lib/forms/state/detail-layout-editor.js';
import { DetailLayoutSaveController } from '#lib/forms/state/detail-layout-save.js';
import { PermissionsController } from '#lib/forms/state/permissions.js';
import { ViewsController } from '#lib/forms/state/views.js';
import { ViewRecordsController } from '#lib/forms/state/view-records.js';
import { ViewDraftController } from '#lib/forms/state/view-draft.js';
import { ViewDraftFieldsController } from '#lib/forms/state/view-draft-fields.js';
import { ViewDraftFiltersController } from '#lib/forms/state/view-draft-filters.js';
import { FilterExpressionExportController } from '#lib/forms/state/filter-expression-export.js';
import { ViewPivotController } from '#lib/forms/state/view-pivot.js';
import { ViewKanbanController } from '#lib/forms/state/view-kanban.js';
import { ViewCalendarController } from '#lib/forms/state/view-calendar.js';
import { ViewGanttController } from '#lib/forms/state/view-gantt.js';
import { ViewTimelineController } from '#lib/forms/state/view-timeline.js';
import { ViewCardsController } from '#lib/forms/state/view-cards.js';
import { ExportController } from '#lib/forms/state/export.js';
import { ImportController } from '#lib/forms/state/import.js';

export interface FormsPageDeps {
	workbench: TaskWorkbench | undefined;
	recordEditorAction: ReturnType<typeof taskEditorAction>;
}

/** Root of the forms page: its dependencies, its state and one controller per stateful module. */
export class FormsPage {
	readonly deps: FormsPageDeps;
	readonly state: FormsPageState;
	readonly session: SessionController;
	readonly loaders: LoadersController;
	readonly records: RecordsController;
	readonly recordPanel: RecordPanelController;
	readonly recordDetail: RecordDetailController;
	readonly attachments: AttachmentsController;
	readonly createForm: CreateFormController;
	readonly designer: DesignerController;
	readonly detailLayoutEditor: DetailLayoutEditorController;
	readonly detailLayoutSave: DetailLayoutSaveController;
	readonly permissions: PermissionsController;
	readonly views: ViewsController;
	readonly viewRecords: ViewRecordsController;
	readonly viewDraft: ViewDraftController;
	readonly viewDraftFields: ViewDraftFieldsController;
	readonly viewDraftFilters: ViewDraftFiltersController;
	readonly filterExpressionExport: FilterExpressionExportController;
	readonly viewPivot: ViewPivotController;
	readonly viewKanban: ViewKanbanController;
	readonly viewCalendar: ViewCalendarController;
	readonly viewGantt: ViewGanttController;
	readonly viewTimeline: ViewTimelineController;
	readonly viewCards: ViewCardsController;
	readonly exporter: ExportController;
	readonly importer: ImportController;

	constructor(deps: FormsPageDeps) {
		this.deps = deps;
		this.state = new FormsPageState(this);
		this.session = new SessionController(this);
		this.loaders = new LoadersController(this);
		this.records = new RecordsController(this);
		this.recordPanel = new RecordPanelController(this);
		this.recordDetail = new RecordDetailController(this);
		this.attachments = new AttachmentsController(this);
		this.createForm = new CreateFormController(this);
		this.designer = new DesignerController(this);
		this.detailLayoutEditor = new DetailLayoutEditorController(this);
		this.detailLayoutSave = new DetailLayoutSaveController(this);
		this.permissions = new PermissionsController(this);
		this.views = new ViewsController(this);
		this.viewRecords = new ViewRecordsController(this);
		this.viewDraft = new ViewDraftController(this);
		this.viewDraftFields = new ViewDraftFieldsController(this);
		this.viewDraftFilters = new ViewDraftFiltersController(this);
		this.filterExpressionExport = new FilterExpressionExportController(this);
		this.viewPivot = new ViewPivotController(this);
		this.viewKanban = new ViewKanbanController(this);
		this.viewCalendar = new ViewCalendarController(this);
		this.viewGantt = new ViewGanttController(this);
		this.viewTimeline = new ViewTimelineController(this);
		this.viewCards = new ViewCardsController(this);
		this.exporter = new ExportController(this);
		this.importer = new ImportController(this);
	}
}
