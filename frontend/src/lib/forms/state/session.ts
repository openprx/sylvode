import { tick } from 'svelte';
import {
	cloneFormsSnapshot,
	formsDraftDirty,
	formsFingerprint,
	type FormsSessionSnapshot,
	mayRefreshForms,
	recordWriteVersion,
	restoredFormsSnapshot
} from '#lib/workbench/forms-session.js';
import { page } from '$app/state';
import { type FormRecord, formsApi, type FormView, type UniversalForm } from '#lib/api/forms.js';
import { projectsApi } from '#lib/api/projects.js';
import { confirmAction } from '#lib/ui/dialog/confirm.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';
import { sortViewsByDefault } from '#lib/forms/view-config.js';

export class SessionController extends FormsController {
	formsDraftGroups = () => {
		return {
			record: { recordDraft: this.s.recordDraft, recordTitle: this.s.recordTitle },
			view: this.s.viewDraft,
			design: {
				designerFieldDrafts: this.s.designerFieldDrafts,
				detailHeaderDraft: this.s.detailHeaderDraft,
				detailHighlightDraft: this.s.detailHighlightDraft,
				detailPageCompositionDraft: this.s.detailPageCompositionDraft,
				detailLayoutSectionDrafts: this.s.detailLayoutSectionDrafts
			},
			create: { formDraft: this.s.formDraft, fieldDrafts: this.s.fieldDrafts },
			comment: this.s.recordCommentDraft,
			child: this.s.childEditor
		};
	};

	cleanFormsGroup = (group: string) => {
		const groups = this.formsDraftGroups();
		this.s.formsSessionClean = {
			...this.s.formsSessionClean,
			[group]: formsFingerprint(groups[group as keyof typeof groups])
		};
	};

	cleanFormsSession = () => {
		this.s.formsSessionClean = Object.fromEntries(
			Object.entries(this.formsDraftGroups()).map(([key, value]) => [key, formsFingerprint(value)])
		);
		this.s.formsSessionSchemaVersion = this.s.selectedForm?.schema_version ?? null;
		this.s.formsSessionViewVersion = this.s.selectedView?.updated_at ?? null;
	};

	formsSessionDirty = () => {
		// View changes are kept automatically (per user, on this device): never unsaved work.
		const groups: Partial<ReturnType<typeof this.formsDraftGroups>> = this.formsDraftGroups();
		delete groups.view;
		return formsDraftDirty(groups, this.s.formsSessionClean);
	};

	formsSessionSaving = () => {
		return (
			this.s.formSubmitting ||
			this.s.viewSubmitting ||
			this.s.recordSubmitting ||
			this.s.designerSaving ||
			this.s.childSubmitting ||
			this.s.detailHeaderSaving ||
			this.s.detailHighlightSaving ||
			this.s.detailPageCompositionSaving ||
			this.s.detailLayoutSectionSaving ||
			this.s.detailPageWidgetSaving ||
			!!this.s.attachmentSubmittingField ||
			this.s.bulkMovingKanban ||
			!!this.s.updatingKanbanRecordId ||
			!!this.s.updatingCalendarRecordId ||
			!!this.s.updatingGanttRecordId ||
			!!this.s.updatingCardRecordId ||
			!!this.s.updatingTimelineRecordId
		);
	};

	formsSessionState = () => {
		return this.s.snapshot({
			selectedViewId: this.s.selectedViewId,
			selectedRecordId: this.s.selectedRecord?.id ?? null,
			focusedRecordIds: this.s.focusedRecordIds,
			activeMode: this.s.activeMode,
			recordPage: this.s.recordPage,
			recordEditorOpen: this.s.recordEditorOpen,
			editingRecordId: this.s.editingRecordId,
			recordDraft: this.s.recordDraft,
			recordTitle: this.s.recordTitle,
			recordCommentDraft: this.s.recordCommentDraft,
			childEditor: this.s.childEditor,
			linkDraft: this.s.linkDraft,
			formDraft: this.s.formDraft,
			fieldDrafts: this.s.fieldDrafts,
			showCreateForm: this.s.showCreateForm,
			viewDraft: this.s.viewDraft,
			viewToolsExpanded: this.s.viewToolsExpanded,
			designerFieldDrafts: this.s.designerFieldDrafts,
			selectedDesignerFieldIndex: this.s.selectedDesignerFieldIndex,
			showAdvancedSchema: this.s.showAdvancedSchema,
			detailLayoutMode: this.s.detailLayoutMode,
			detailHeaderDraft: this.s.detailHeaderDraft,
			detailHighlightDraft: this.s.detailHighlightDraft,
			detailPageCompositionDraft: this.s.detailPageCompositionDraft,
			detailLayoutSectionDrafts: this.s.detailLayoutSectionDrafts,
			collapsedCardGroupKeys: this.s.collapsedCardGroupKeys,
			selectedKanbanRecordIds: this.s.selectedKanbanRecordIds,
			pinnedTimelineEventIds: this.s.pinnedTimelineEventIds,
			formsSessionConflict: this.s.formsSessionConflict,
			focus: this.s.formsSessionFocus,
			scroll: Array.from(
				this.s.formsSessionRoot?.querySelectorAll<HTMLElement>(
					'.overflow-x-auto,.overflow-auto,.overflow-y-auto'
				) ?? []
			).map((element) => ({ left: element.scrollLeft, top: element.scrollTop }))
		});
	};

	captureFormsSession = (): FormsSessionSnapshot => {
		return cloneFormsSnapshot({
			formId: this.s.selectedFormId || null,
			state: this.formsSessionState(),
			clean: this.s.snapshot(this.s.formsSessionClean),
			formSchemaVersion: this.s.formsSessionSchemaVersion,
			recordVersion: this.s.snapshot(this.s.formsSessionRecordVersion),
			viewUpdatedAt: this.s.formsSessionViewVersion
		});
	};

	releaseFormsSession = () => {
		this.s.formsSessionRelease?.();
		this.s.formsSessionRelease = null;
		this.s.formsSessionKey = null;
		this.s.formsSessionUser = null;
	};

	attachFormsSession = async () => {
		const workbench = this.s.workbench;

		const user = workbench?.current.userId;
		const key = this.s.selectedFormId
			? `forms:${this.s.projectId}:${this.s.selectedFormId}`
			: `forms-entry:${this.s.projectId}`;
		if (!workbench || !user || (this.s.formsSessionKey === key && this.s.formsSessionUser === user))
			return;
		// Never render a cached snapshot as authority. The live resource read must succeed first.
		const id = this.s.selectedFormId;
		const generation = this.s.formsSessionGeneration;
		const authorized = id ? await formsApi.get(id) : await projectsApi.get(this.s.projectId);
		if (
			authorized.code !== 0 ||
			!authorized.data ||
			workbench.current.userId !== user ||
			generation !== this.s.formsSessionGeneration ||
			id !== this.s.selectedFormId
		)
			return;
		const urlForm = page.url.searchParams.get('form') ?? page.url.searchParams.get('form_id');
		if (urlForm && urlForm !== id) return;
		const cached = restoredFormsSnapshot(
			workbench.snapshot(key),
			this.s.selectedFormId || null,
			true
		);
		if (cached) {
			const saved = cached.state as ReturnType<typeof this.formsSessionState>;
			this.s.selectedRecord =
				this.s.records.find((row) => row.id === saved.selectedRecordId) ?? null;
			if (this.s.selectedRecord)
				await Promise.all([
					this.fp.recordPanel.loadSelectedRecordLinks(),
					this.fp.recordPanel.loadSelectedRecordChildren(),
					this.fp.recordPanel.loadSelectedRecordComments(),
					this.fp.attachments.loadRecordAttachments(this.s.selectedRecord.id)
				]);
			if (generation !== this.s.formsSessionGeneration || workbench.current.userId !== user) return;
			({
				selectedViewId: this.s.selectedViewId,
				focusedRecordIds: this.s.focusedRecordIds,
				activeMode: this.s.activeMode,
				recordPage: this.s.recordPage,
				recordEditorOpen: this.s.recordEditorOpen,
				editingRecordId: this.s.editingRecordId,
				recordDraft: this.s.recordDraft,
				recordTitle: this.s.recordTitle,
				recordCommentDraft: this.s.recordCommentDraft,
				childEditor: this.s.childEditor,
				linkDraft: this.s.linkDraft,
				formDraft: this.s.formDraft,
				fieldDrafts: this.s.fieldDrafts,
				showCreateForm: this.s.showCreateForm,
				viewDraft: this.s.viewDraft,
				viewToolsExpanded: this.s.viewToolsExpanded,
				designerFieldDrafts: this.s.designerFieldDrafts,
				selectedDesignerFieldIndex: this.s.selectedDesignerFieldIndex,
				showAdvancedSchema: this.s.showAdvancedSchema,
				detailLayoutMode: this.s.detailLayoutMode,
				detailHeaderDraft: this.s.detailHeaderDraft,
				detailHighlightDraft: this.s.detailHighlightDraft,
				detailPageCompositionDraft: this.s.detailPageCompositionDraft,
				detailLayoutSectionDrafts: this.s.detailLayoutSectionDrafts,
				collapsedCardGroupKeys: this.s.collapsedCardGroupKeys,
				selectedKanbanRecordIds: this.s.selectedKanbanRecordIds,
				pinnedTimelineEventIds: this.s.pinnedTimelineEventIds,
				formsSessionConflict: this.s.formsSessionConflict
			} = saved);
			if (this.s.activeMode === 'detail') this.s.activeMode = 'data';
			this.s.formsSessionFocus = saved.focus ?? null;
			this.s.formsSessionClean = cached.clean;
			this.s.formsSessionSchemaVersion = cached.formSchemaVersion;
			this.s.formsSessionRecordVersion = cached.recordVersion;
			this.s.formsSessionViewVersion = cached.viewUpdatedAt;
			this.s.formsSessionRemoteRecord =
				this.s.records.find((row) => row.id === cached.recordVersion?.recordId) ?? null;
			this.s.formsSessionRemoteSchema = this.s.selectedForm?.schema_version ?? null;
			this.s.formsSessionRemoteChanged = !!(
				cached.recordVersion &&
				this.s.formsSessionRemoteRecord &&
				(cached.recordVersion.updatedAt !== this.s.formsSessionRemoteRecord.updated_at ||
					cached.recordVersion.schemaVersion !== this.s.formsSessionRemoteSchema)
			);
		}
		this.s.formsSessionKey = key;
		this.s.formsSessionUser = user;
		this.s.formsSessionRelease = workbench.register(key, {
			capture: this.captureFormsSession,
			dirty: this.formsSessionDirty,
			refresh: this.refreshFormsSession
		});
		workbench.setTitle(key, this.s.selectedForm?.name ?? this.s.project?.name ?? 'Forms');
		if (cached) {
			await tick();
			if (generation !== this.s.formsSessionGeneration || key !== this.s.formsSessionKey) return;
			const scroll = (cached.state as ReturnType<typeof this.formsSessionState>).scroll;
			this.s.formsSessionRoot
				?.querySelectorAll<HTMLElement>('.overflow-x-auto,.overflow-auto,.overflow-y-auto')
				.forEach((element, index) => {
					const position = scroll?.[index];
					if (position) {
						element.scrollLeft = position.left;
						element.scrollTop = position.top;
					}
				});
			const focus = this.s.formsSessionFocus;
			if (focus) {
				const input = this.s.formsSessionRoot?.querySelector<
					HTMLInputElement | HTMLTextAreaElement
				>(focus.selector);
				input?.focus({ preventScroll: true });
				try {
					if (input && focus.start !== null) input.setSelectionRange(focus.start, focus.end);
				} catch {
					/* Nontext controls retain focus only. */
				}
			}
		}
	};

	refreshFormsSession = async () => {
		const workbench = this.s.workbench;

		const key = this.s.formsSessionKey;
		const user = this.s.formsSessionUser;
		const id = this.s.selectedFormId;
		if (
			!key ||
			!user ||
			!workbench ||
			!mayRefreshForms({
				active: workbench.current.active === key,
				visible: document.visibilityState === 'visible',
				online: navigator.onLine,
				saving: this.formsSessionSaving(),
				loading: this.s.loading
			})
		)
			return;
		await this.s.formsSessionLatch.run(async () => {
			const generation = this.s.formsSessionGeneration;
			try {
				const [freshForms, freshForm, freshViews, freshRecords] = await Promise.all([
					formsApi.list(this.s.projectId, { page: 1, per_page: 100 }),
					id ? formsApi.get(id) : projectsApi.get(this.s.projectId),
					id ? formsApi.listViews(id) : Promise.resolve({ code: 0, data: [] as FormView[] }),
					id
						? formsApi.listRecords(id, { page: 1, per_page: 100 })
						: Promise.resolve({ code: 0, data: { items: [] as FormRecord[] } })
				]);
				if (
					generation !== this.s.formsSessionGeneration ||
					key !== this.s.formsSessionKey ||
					workbench.current.userId !== user
				)
					return;
				if (
					[freshForms, freshForm, freshViews, freshRecords].some(
						(result) => result.code !== 0 || !result.data
					)
				) {
					this.s.formsSessionRefreshFailed = true;
					return;
				}
				this.s.formsSessionRefreshFailed = false;
				const rows = freshRecords.data?.items ?? [];
				const currentForm = id ? (freshForm.data as UniversalForm) : null;
				this.s.formsSessionRemoteRecord =
					rows.find((row) => row.id === this.s.editingRecordId) ?? null;
				this.s.formsSessionRemoteSchema = currentForm?.schema_version ?? null;
				const changed =
					formsFingerprint({
						forms: freshForms.data?.items,
						views: freshViews.data,
						records: rows
					}) !==
					formsFingerprint({
						forms: this.s.forms,
						views: this.s.formViews,
						records: this.s.records
					});
				if (
					this.formsSessionDirty() ||
					this.s.editingRecordId !== null ||
					this.s.recordEditorOpen
				) {
					if (changed) {
						this.s.formsSessionRemoteChanged = true;
						workbench.markChanged(key);
					}
					return; // Whole editing snapshot and its original CAS remain intact.
				}
				this.s.forms = freshForms.data?.items ?? [];
				if (!id) return;
				this.s.formViews = sortViewsByDefault(freshViews.data ?? []);
				this.s.records = rows;
				this.s.filterPreviewRecords = rows;
				if (this.s.selectedRecord)
					this.s.selectedRecord = rows.find((row) => row.id === this.s.selectedRecord?.id) ?? null;
				// Retain local filters/selection; only replace clean schema/design baselines.
				this.s.designerSyncedFormId = '';
				this.fp.designer.syncDesignerDrafts(currentForm);
				this.cleanFormsGroup('design');
				this.s.formsSessionSchemaVersion = currentForm?.schema_version ?? null;
				this.s.formsSessionRemoteChanged = false;
			} catch {
				if (generation === this.s.formsSessionGeneration) this.s.formsSessionRefreshFailed = true;
			}
		});
	};

	updateRecordWithVersion = async (
		recordId: string,
		data: Parameters<typeof formsApi.updateRecord>[1]
	) => {
		const record =
			this.s.records.find((row) => row.id === recordId) ??
			(this.s.selectedRecord?.id === recordId ? this.s.selectedRecord : null);
		const version = recordWriteVersion(
			recordId,
			this.s.formsSessionRecordVersion,
			record,
			this.s.formsSessionSchemaVersion ?? this.s.selectedForm?.schema_version ?? null
		);
		if (!version) return { code: 409, message: translate('forms.session.conflict'), data: null };
		const result = await formsApi.updateRecord(recordId, { ...data, ...version });
		if (result.code === 409) {
			this.s.formsSessionConflict = true;
			this.s.formsSessionRemoteChanged = true;
			window.setTimeout(() => {
				void this.refreshFormsSession();
			}, 0);
		}
		return result;
	};

	reloadFormsSession = async () => {
		const workbench = this.s.workbench;

		if (
			this.formsSessionDirty() &&
			!(await confirmAction({
				tone: 'destructive',
				title: translate('forms.dialogs.discardChanges.title'),
				body: translate('forms.dialogs.discardChanges.body'),
				confirmLabel: translate('forms.dialogs.discardChanges.confirm')
			}))
		)
			return;
		const id = this.s.selectedFormId;
		const recordId = this.s.editingRecordId;
		this.s.formsSessionGeneration++;
		const fresh = await formsApi.get(id);
		if (fresh.code !== 0 || !fresh.data) {
			this.s.formsSessionRefreshFailed = true;
			return;
		}
		this.s.forms = this.s.forms.map((item) => (item.id === id ? fresh.data! : item));
		const loaded = await this.fp.loaders.loadFormState(id, { syncUrl: false });
		if (!loaded) {
			this.s.formsSessionRefreshFailed = true;
			return;
		}
		const record =
			this.s.records.find((row) => row.id === recordId) ??
			this.s.filterPreviewRecords.find((row) => row.id === recordId);
		if (record) this.fp.records.openRecordEditDrawer(record);
		this.cleanFormsSession();
		this.s.formsSessionConflict = false;
		this.s.formsSessionRemoteChanged = false;
		this.s.formsSessionReview = false;
		this.s.formsSessionRefreshFailed = false;
		this.s.formsSessionRemoteRecord = null;
		this.s.formsSessionRemoteSchema = null;
		if (this.s.formsSessionKey) {
			workbench?.markChanged(this.s.formsSessionKey, false);
			workbench?.markDirty(this.s.formsSessionKey, false);
		}
	};

	openForm = async (formId: string, options: { syncUrl?: boolean } = {}) => {
		const workbench = this.s.workbench;

		const target = new URL(page.url.href);
		target.searchParams.set('form', formId);
		target.searchParams.delete('form_id');
		if (workbench?.current.userId && !workbench.canOpen(target.href)) {
			workbench.block(target.href);
			return;
		}
		if (this.s.formsSessionActivating === formId) return;
		const request = ++this.s.formsSessionOpenRequest;
		if (this.s.formsSessionOpening) await this.s.formsSessionOpening;
		if (request !== this.s.formsSessionOpenRequest) return;
		const opening = (async () => {
			this.releaseFormsSession();
			this.s.formsSessionGeneration++;
			this.s.formsSessionActivating = formId;
			this.s.formsSessionTransitioning = true;
			this.s.formsSessionConflict = false;
			this.s.formsSessionRemoteChanged = false;
			this.s.formsSessionRefreshFailed = false;
			this.s.formsSessionReview = false;
			try {
				if (options.syncUrl !== false) {
					await this.fp.loaders.syncFormUrl(formId);
					const admitted =
						page.url.searchParams.get('form') ?? page.url.searchParams.get('form_id');
					if (admitted !== formId) return;
				}
				await this.fp.loaders.loadFormState(formId, { syncUrl: false });
				if (this.s.formsSessionReady) await this.fp.recordPanel.restoreRecordDetailFromUrl();
				this.cleanFormsSession();
				if (this.s.formsSessionReady) await this.attachFormsSession();
			} finally {
				this.s.formsSessionActivating = '';
				this.s.formsSessionTransitioning = false;
			}
		})();
		this.s.formsSessionOpening = opening;
		try {
			await opening;
		} finally {
			if (this.s.formsSessionOpening === opening) this.s.formsSessionOpening = null;
		}
	};

	rememberFormsFocus = (event: FocusEvent) => {
		const input = event.target;
		if (!(input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement)) return;
		const field = input.closest<HTMLElement>('[data-record-edit-field]')?.dataset.recordEditField;
		const selector = field
			? `[data-record-edit-field="${CSS.escape(field)}"] ${input.tagName.toLowerCase()}`
			: input.id
				? `#${CSS.escape(input.id)}`
				: null;
		if (selector)
			this.s.formsSessionFocus = { selector, start: input.selectionStart, end: input.selectionEnd };
	};
}
