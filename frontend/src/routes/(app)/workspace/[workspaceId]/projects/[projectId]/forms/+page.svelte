<script lang="ts">
	import { taskEditorAction } from '#lib/workbench/edit-surface.js';
	import { afterNavigate } from '$app/navigation';
	import { getContext, onMount, tick, untrack } from 'svelte';
	import { WORKBENCH_CONTEXT, type TaskWorkbench } from '#lib/workbench/tabs.js';
	import { page } from '$app/state';
	import * as arrangement from '#lib/forms/view-arrangement.js';
	import { t } from 'svelte-i18n';
	import { FormsPage } from '#lib/forms/state/forms-page.js';
	import { setFormsPageContext } from '#lib/forms/state/context.js';
	import FormsSessionNotice from '#lib/components/forms/page/FormsSessionNotice.svelte';
	import FormsPageHeader from '#lib/components/forms/page/FormsPageHeader.svelte';
	import FormsIndex from '#lib/components/forms/page/FormsIndex.svelte';
	import FormCreatePanel from '#lib/components/forms/page/FormCreatePanel.svelte';
	import ImportWizardModal from '#lib/components/forms/page/ImportWizardModal.svelte';
	import DesignerConfirmModal from '#lib/components/forms/page/DesignerConfirmModal.svelte';
	import FormWorkspace from '#lib/components/forms/page/FormWorkspace.svelte';
	import ProjectAppsStrip from '#lib/components/plugins/ProjectAppsStrip.svelte';

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
		openForm,
		rememberFormsFocus
	} = fp.session;
	const { loadProject, loadScenarioTemplates, loadWorkspaceMembers, loadForms, loadRecords } =
		fp.loaders;
	const { handleGlobalKeydown } = fp.records;
	const { restoreRecordDetailFromUrl, openRecordPanel } = fp.recordPanel;

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
	{#if page.params.workspaceId && page.params.projectId}
		<ProjectAppsStrip workspaceId={page.params.workspaceId} projectId={page.params.projectId} />
	{/if}

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
