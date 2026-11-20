import { goto } from '$app/navigation';
import { page } from '$app/state';
import type { ApiResult } from '#lib/api/client.js';
import { formsApi, type FormView } from '#lib/api/forms.js';
import type { SimpleFilter } from '#lib/components/forms/ViewArrangementControls.svelte';
import * as arrangement from '#lib/forms/view-arrangement.js';
import * as precedence from '#lib/forms/view-precedence.js';
import { toast } from '#lib/stores/toast.js';
import { SORTABLE_TYPES } from '#lib/forms/page-constants.js';
import { fieldLabel } from '#lib/forms/field-options.js';
import { translate } from '#lib/forms/i18n.js';
import { FormsController } from '#lib/forms/state/controller.js';
import {
	builtInView,
	normalizeLocalDefaultViews,
	sortViewsByDefault,
	viewIsDefault
} from '#lib/forms/view-config.js';
import { syncLegacyViewFilterFields } from '#lib/forms/view-draft-parse.js';

export class ViewsController extends FormsController {
	isServerSortable = (key: string): boolean => {
		if (['title', 'created_at', 'updated_at'].includes(key)) return true;
		return SORTABLE_TYPES.has(this.fp.viewRecords.fieldByKey(key)?.type ?? '');
	};

	/** Applies a view (saved or built-in) and brings back this user's working changes to it. */
	applyView = (viewId: string) => {
		this.s.selectedViewId = viewId;
		const view =
			this.s.formViews.find((item) => item.id === viewId) ??
			(this.s.selectedForm ? builtInView(this.s.selectedForm) : null);
		if (!view) return;
		this.fp.viewDraft.resetViewDraft(view);
		const config = this.fp.viewDraft.viewConfigFromDraft();
		this.s.viewBaseline = {
			viewId: view.id,
			type: this.s.viewDraft.view_type,
			key: arrangement.normalizedViewConfig(config),
			config
		};
		const stored = arrangement.readArrangement(
			this.s.currentUserId,
			this.s.selectedFormId,
			view.id
		);
		if (!stored) return;
		if (stored.base_updated_at !== (this.s.savedView ? view.updated_at : null)) {
			arrangement.clearArrangement(this.s.currentUserId, this.s.selectedFormId, view.id);
			toast.info(translate('forms.views.updatedElsewhere', { values: { name: view.name } }));
			return;
		}
		const audience = { visibility: view.config?.visibility, is_default: view.config?.is_default };
		this.fp.viewDraft.resetViewDraft({
			...view,
			view_type: stored.view_type,
			config: { ...stored.config, ...audience }
		});
	};

	/** Switches the view; `remember` marks an explicit choice (kept per user and form, put in the URL). */
	showView = (viewId: string, options: { remember: boolean }) => {
		this.applyView(viewId);
		this.s.viewConflict = false;
		this.s.resetUndo = null;
		if (!options.remember) return;
		precedence.writeRememberedView(this.s.currentUserId, this.s.selectedFormId, viewId);
		const url = new URL(page.url.href);
		url.searchParams.set('view', viewId);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- same route, query only
		void goto(url, { replace: true, reset: false });
	};

	resetArrangement = () => {
		if (!this.s.baseView) return;
		clearTimeout(this.s.arrangementTimer);
		const undo = {
			base_updated_at: null,
			view_type: this.s.viewDraft.view_type,
			config: this.s.draftViewConfig,
			at: ''
		};
		arrangement.clearArrangement(this.s.currentUserId, this.s.selectedFormId, this.s.baseView.id);
		this.applyView(this.s.baseView.id);
		this.s.resetUndo = undo;
		this.fp.recordPanel.announce(translate('forms.views.resetDone'));
		setTimeout(() => {
			if (this.s.resetUndo === undo) this.s.resetUndo = null;
		}, 6000);
	};

	undoReset = () => {
		const undo = this.s.resetUndo;
		if (!undo || !this.s.baseView) return;
		this.s.resetUndo = null;
		this.fp.viewDraft.resetViewDraft({
			...this.s.baseView,
			view_type: undo.view_type,
			config: undo.config
		});
	};

	writeView = async (request: Promise<ApiResult<FormView>>): Promise<FormView | null> => {
		this.s.viewSubmitting = true;
		const response = await request;
		this.s.viewSubmitting = false;
		if (response.code === 409) this.s.viewConflict = true;
		if (response.code !== 0 || !response.data) {
			if (response.code !== 409) toast.error(response.message);
			return null;
		}
		const view = response.data;
		const next = this.s.formViews.some((item) => item.id === view.id)
			? this.s.formViews.map((item) => (item.id === view.id ? view : item))
			: [...this.s.formViews, view];
		this.s.formViews = viewIsDefault(view)
			? normalizeLocalDefaultViews(next, view.id)
			: sortViewsByDefault(next);
		return view;
	};

	saveViewAs = async (input: { name: string; visibility: 'private' | 'shared' }) => {
		if (!this.s.selectedForm || !this.s.baseView) return;
		this.s.saveViewError = null;
		const from = this.s.baseView.id;
		const config = { ...this.s.draftViewConfig, visibility: input.visibility, is_default: false };
		const key = arrangement.generateViewKey(input.name);
		const request = formsApi.createView(this.s.selectedForm.id, {
			key,
			name: input.name,
			view_type: this.s.viewDraft.view_type,
			config
		});
		const saving = await request;
		if (saving.code !== 0 || !saving.data) {
			this.s.saveViewError = saving.message;
			return;
		}
		await this.writeView(Promise.resolve(saving));
		arrangement.clearArrangement(this.s.currentUserId, this.s.selectedFormId, from);
		this.s.saveViewOpen = false;
		this.showView(saving.data.id, { remember: true });
		this.fp.recordPanel.announce(translate('forms.views.viewSaved'));
		toast.success(translate('forms.views.viewSaved'));
	};

	/** Updates the saved view: with the working changes, or with a name or audience change only. */
	updateCurrentView = async (patch?: { name?: string; config?: Record<string, unknown> }) => {
		const view = this.s.savedView;
		if (!view) return;
		const audience = {
			visibility: view.config?.visibility ?? 'shared',
			is_default: viewIsDefault(view)
		};
		const body = patch ?? {
			view_type: this.s.viewDraft.view_type,
			config: { ...this.s.draftViewConfig, ...audience }
		};
		const updated = await this.writeView(
			formsApi.updateView(view.id, { ...body, expected_updated_at: view.updated_at })
		);
		if (!updated) return;
		this.s.viewConflict = false;
		if (!patch) {
			arrangement.clearArrangement(this.s.currentUserId, this.s.selectedFormId, view.id);
			this.applyView(view.id);
		}
		toast.success(translate('forms.views.viewUpdated'));
	};

	deleteView = async () => {
		const view = this.s.savedView;
		if (!view) return;
		const response = await formsApi.deleteView(view.id);
		if (response.code !== 0) {
			toast.error(response.message);
			return;
		}
		this.s.formViews = this.s.formViews.filter((item) => item.id !== view.id);
		arrangement.clearArrangement(this.s.currentUserId, this.s.selectedFormId, view.id);
		if (precedence.readRememberedView(this.s.currentUserId, this.s.selectedFormId) === view.id)
			precedence.forgetRememberedView(this.s.currentUserId, this.s.selectedFormId);
		this.showView(precedence.ALL_RECORDS_VIEW_ID, { remember: false });
		toast.success(translate('forms.views.viewDeleted'));
	};

	openSaveView = (name?: string) => {
		this.s.saveViewError = null;
		this.s.saveViewName =
			name ??
			arrangement.suggestViewName(
				this.s.baseView ?? { name: '' },
				this.s.draftViewConfig,
				(key) => {
					const field = this.fp.viewRecords.fieldByKey(key);
					return field
						? fieldLabel(field)
						: translate(
								`forms.views.${key === 'title' ? 'titleField' : key === 'created_at' ? 'createdField' : 'updatedField'}`
							);
				},
				(field) => translate('forms.views.byField', { values: { field } })
			);
		this.s.saveViewOpen = true;
	};

	openLayoutSettings = () => {
		if (this.s.panelRecordId) this.fp.recordPanel.closeRecordPanel(false);
		this.s.viewToolsExpanded = true;
	};

	setDraftFilters = (filters: SimpleFilter[], logic: 'all' | 'any') => {
		const filterGroups = filters.length > 0 ? [{ logic, filters }] : [];
		this.s.viewDraft = {
			...this.s.viewDraft,
			filterGroups,
			...syncLegacyViewFilterFields(filterGroups)
		};
	};
}
