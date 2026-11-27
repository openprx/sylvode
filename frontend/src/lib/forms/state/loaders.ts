import { describeResult } from '#lib/i18n/describe.js';
import { goto } from '$app/navigation';
import { page } from '$app/state';
import { type FormAggregate, formsApi } from '#lib/api/forms.js';
import { membersApi } from '#lib/api/members.js';
import { projectsApi } from '#lib/api/projects.js';
import * as precedence from '#lib/forms/view-precedence.js';
import { toast } from '#lib/stores/toast.js';
import { defaultPermissionActions } from '#lib/forms/permission-drafts.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { sortViewsByDefault } from '#lib/forms/view-config.js';

export class LoadersController extends FormsController {
	loadProject = async () => {
		const response = await projectsApi.get(this.s.projectId);

		if (response.code === 0 && response.data) {
			this.s.project = response.data;
		}
	};

	loadTemplatePackages = async () => {
		this.s.templatesLoading = true;

		const response = await projectsApi.listTemplatePackages(this.s.workspaceId);

		this.s.templatesLoading = false;

		if (response.code !== 0) {
			toast.error(describeResult(response));
			this.s.templatePackages = [];

			return;
		}

		this.s.templatePackages = (response.data?.items ?? []).filter(
			(item) => item.tier === 'builtin'
		);
	};

	loadWorkspaceMembers = async () => {
		this.s.workspaceMembersLoading = true;

		const response = await membersApi.list(this.s.workspaceId);

		this.s.workspaceMembersLoading = false;

		if (response.code !== 0) {
			this.s.workspaceMembers = [];
			toast.error(describeResult(response));

			return;
		}

		this.s.workspaceMembers = response.data?.items ?? [];
	};

	loadForms = async () => {
		const response = await formsApi.list(this.s.projectId, { page: 1, per_page: 100 });

		if (response.code !== 0) {
			toast.error(describeResult(response));
			this.s.forms = [];

			return;
		}

		this.s.forms = response.data?.items ?? [];

		const urlFormId = page.url.searchParams.get('form') ?? page.url.searchParams.get('form_id');

		if (urlFormId && this.s.forms.some((form) => form.id === urlFormId)) {
			await this.fp.session.openForm(urlFormId, { syncUrl: false });
		} else {
			this.clearSelectedFormState();
		}

		await this.loadPrintJobs();
	};

	clearSelectedFormState = () => {
		this.s.selectedFormId = '';
		this.s.records = [];
		this.s.filterPreviewRecords = [];
		this.s.formViews = [];
		this.s.formEvents = [];
		this.s.formPermissions = null;
		this.s.memberPermissionDraft = defaultPermissionActions();
		this.s.memberFieldReadDraft = {};
		this.s.memberFieldWriteDraft = {};
		this.s.selectedViewId = precedence.ALL_RECORDS_VIEW_ID;
		this.s.panelRecordId = null;
		this.s.selectedRecord = null;
		this.s.selectedRecordLinks = [];
		this.s.selectedRecordChildren = [];
		this.s.selectedRecordComments = [];
		this.s.recordCommentDraft = '';
		this.s.recordAttachments = [];
		this.s.relationTargets = {};
		this.s.designerFieldDrafts = [];
		this.s.designerSyncedFormId = '';
		this.s.activeMode = 'data';
		this.s.editingRecordId = null;
		this.s.recordEditorOpen = false;
		this.s.recordPage = 1;
	};

	syncFormUrl = async (formId: string) => {
		const url = new URL(page.url.href);

		url.searchParams.set('form', formId);
		url.searchParams.delete('form_id');
		url.searchParams.delete('record');
		url.searchParams.delete('record_id');
		url.searchParams.delete('view');

		const next = `${url.pathname}${url.search}${url.hash}`;
		const current = `${page.url.pathname}${page.url.search}${page.url.hash}`;

		if (next !== current) {
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- rebuilds the current same-app URL
			await goto(next, { replaceState: true, reset: false });
		}
	};

	loadFormState = async (formId: string, options: { syncUrl?: boolean } = {}) => {
		const form = this.s.forms.find((item) => item.id === formId);

		if (!form) return false;

		this.s.selectedFormId = form.id;
		this.s.selectedViewId = precedence.ALL_RECORDS_VIEW_ID;
		this.s.panelRecordId = null;
		this.s.editingRecordId = null;
		this.s.recordEditorOpen = false;
		this.s.recordPage = 1;

		// Record ids belong to the form they were focused from; keeping them
		// across a form switch filters every view down to zero rows.
		this.s.focusedRecordIds = [];

		this.s.lastDeletedRecord = null;
		this.s.selectedRecord = null;
		this.s.selectedRecordLinks = [];
		this.s.selectedRecordChildren = [];
		this.s.selectedRecordComments = [];
		this.s.recordCommentDraft = '';
		this.s.recordAttachments = [];
		this.s.relationTargets = {};
		this.s.formViews = [];
		this.s.formPermissions = null;
		this.s.formEvents = [];
		this.s.activeMode = 'data';
		this.fp.designer.syncDesignerDrafts(form);
		this.fp.detailLayoutEditor.syncDetailHeaderDraft(form);
		this.fp.detailLayoutEditor.syncDetailHighlightDraft(form);
		this.fp.detailLayoutEditor.syncDetailPageCompositionDraft(form);
		this.fp.detailLayoutEditor.syncDetailLayoutSectionDrafts(form);

		if (options.syncUrl !== false) await this.syncFormUrl(form.id);

		await this.fp.permissions.loadFormPermissions();
		await this.loadFormViews();
		return await this.loadRecords();
	};

	loadFormViews = async () => {
		if (!this.s.selectedFormId) {
			this.s.formViews = [];
			this.s.selectedViewId = precedence.ALL_RECORDS_VIEW_ID;

			return;
		}

		const response = await formsApi.listViews(this.s.selectedFormId);

		if (response.code !== 0) {
			toast.error(describeResult(response));
			this.s.formViews = [];
			this.s.selectedViewId = precedence.ALL_RECORDS_VIEW_ID;
			this.fp.views.applyView(precedence.ALL_RECORDS_VIEW_ID);

			return;
		}

		this.s.formViews = sortViewsByDefault(response.data ?? []);
		const urlForm = page.url.searchParams.get('form') ?? page.url.searchParams.get('form_id');
		const resolved = precedence.resolveInitialView({
			views: this.s.formViews,
			urlViewId: urlForm === this.s.selectedFormId ? page.url.searchParams.get('view') : null,
			rememberedViewId: precedence.readRememberedView(this.s.currentUserId, this.s.selectedFormId)
		});
		if (resolved.dropRemembered)
			precedence.forgetRememberedView(this.s.currentUserId, this.s.selectedFormId);
		this.fp.views.applyView(resolved.viewId);
	};

	loadRecords = async () => {
		if (!this.s.selectedFormId) return false;

		this.s.recordsLoading = true;
		const query = this.s.recordQuery;
		this.s.loadedRecordQuery = JSON.stringify([this.s.selectedFormId, query]);

		const [response, previewResponse] = await Promise.all([
			formsApi.listRecords(this.s.selectedFormId, {
				page: 1,
				per_page: query.per_page,
				view_id: query.view_id,
				sort: query.sort
			}),
			formsApi.listRecords(this.s.selectedFormId, { page: 1, per_page: 100 })
		]);

		if (response.code !== 0) {
			toast.error(describeResult(response));
			this.s.records = [];
		} else {
			this.s.records = response.data?.items ?? [];
			this.s.serverRecordTotal = response.data?.total ?? this.s.records.length;

			if (
				this.s.selectedRecord &&
				this.s.selectedRecord.id !== this.s.panelRecordId &&
				!this.s.records.some((record) => record.id === this.s.selectedRecord?.id)
			) {
				this.s.selectedRecord = null;
				this.s.selectedRecordLinks = [];
				this.s.selectedRecordChildren = [];
				this.s.selectedRecordComments = [];
				void this.fp.recordPanel.syncRecordDetailUrl(null);
			}
		}

		if (previewResponse.code === 0) {
			this.s.filterPreviewRecords = previewResponse.data?.items ?? [];
		} else {
			this.s.filterPreviewRecords = this.s.records;
		}

		await this.loadAggregates();
		await this.loadRelationTargets();
		await this.loadFormEvents();
		this.fp.records.resetRecordDraft();
		this.s.recordsLoading = false;
		return response.code === 0 && previewResponse.code === 0;
	};

	loadFormEvents = async () => {
		if (!this.s.selectedFormId) {
			this.s.formEvents = [];

			return;
		}

		this.s.eventsLoading = true;

		const response = await formsApi.listEvents(this.s.selectedFormId);

		this.s.eventsLoading = false;

		if (response.code !== 0) {
			this.s.formEvents = [];
			toast.error(describeResult(response));

			return;
		}

		this.s.formEvents = response.data?.items ?? [];
	};

	loadPrintJobs = async () => {
		if (!this.s.printJobForm) {
			this.s.printJobs = [];

			return;
		}

		const response = await formsApi.listRecords(this.s.printJobForm.id, { page: 1, per_page: 8 });

		if (response.code === 0 && response.data) {
			this.s.printJobs = response.data.items ?? [];
		}
	};

	loadRelationTargets = async () => {
		if (!this.s.selectedForm) {
			this.s.relationTargets = {};

			return;
		}

		const relationFields = this.s.fields.filter(
			(field) => field.type === 'relation' && field.relation?.target_type !== 'external'
		);

		if (relationFields.length === 0) {
			this.s.relationTargets = {};

			return;
		}

		this.s.relationTargetsLoading = true;

		const selectedForm = this.s.selectedForm;
		const entries = await Promise.all(
			relationFields.map(async (field) => {
				const response = await formsApi.listRelationTargets(selectedForm.id, {
					field_key: field.key,
					per_page: 100
				});

				return [
					field.key,
					response.code === 0 && response.data ? response.data.items : []
				] as const;
			})
		);

		this.s.relationTargets = Object.fromEntries(entries);
		this.s.relationTargetsLoading = false;
	};

	loadAggregates = async () => {
		if (!this.s.selectedFormId || !this.s.selectedForm) {
			this.s.aggregates = {};

			return;
		}

		const fieldsToAggregate = parseFields(this.s.selectedForm.schema).filter((field) =>
			['amount', 'number', 'integer'].includes(field.type ?? 'text')
		);

		const results = await Promise.all(
			fieldsToAggregate.map(async (field) => {
				const response = await formsApi.aggregate(this.s.selectedFormId, field.key, 'sum');

				return [field.key, response.code === 0 && response.data ? response.data : null] as const;
			})
		);

		this.s.aggregates = Object.fromEntries(
			results.filter((entry): entry is readonly [string, FormAggregate] => Boolean(entry[1]))
		);
	};
}
