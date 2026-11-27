<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import LayoutSettingsSheet from '#lib/components/forms/LayoutSettingsSheet.svelte';
	import RecordSidePanel from '#lib/components/forms/RecordSidePanel.svelte';
	import LayoutSettingsContent from '#lib/components/forms/page/LayoutSettingsContent.svelte';
	import RecordEditorDrawer from '#lib/components/forms/page/RecordEditorDrawer.svelte';
	import RecordListPanel from '#lib/components/forms/page/RecordListPanel.svelte';
	import { buildDetailSections, headerFields } from '#lib/forms/detail-layout.js';
	import {
		fieldHasConditionalHidden,
		fieldHasConditionalReadOnly,
		recordFieldHiddenByCondition
	} from '#lib/forms/field-conditions.js';
	import * as arrangement from '#lib/forms/view-arrangement.js';
	import * as precedence from '#lib/forms/view-precedence.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleDeleteRecord, handleRestoreLastDeletedRecord, openRecordEditDrawer } = fp.records;
	const {
		loadSelectedRecordComments,
		reloadSelectedRecord,
		announce,
		recordHref,
		canChangeRecord,
		openRecordPanel,
		closeRecordPanel,
		stepRecordPanel
	} = fp.recordPanel;
	const { childRecordsForRelationField, childGridFieldsForRelationField } = fp.recordDetail;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const { showView } = fp.views;
</script>

<div class="flex min-w-0 items-start gap-4" data-record-list-stack>
	<LayoutSettingsSheet
		open={s.viewToolsExpanded}
		viewName={s.selectedView?.name ?? ''}
		onclose={() => (s.viewToolsExpanded = false)}
	>
		<LayoutSettingsContent />
	</LayoutSettingsSheet>

	{#if s.recordEditorOpen}
		<RecordEditorDrawer />
	{/if}

	<RecordListPanel />
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
			onrecalculate={() => void fp.recordPanel.recalculatePanelRecord()}
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
