<svelte:options runes />

<script lang="ts">
	import FormDesignerWorkspace from '#lib/components/forms/FormDesignerWorkspace.svelte';
	import { fieldTypeOptions } from '#lib/forms/page-constants.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		addDesignerField,
		updateDesignerField,
		duplicateDesignerField,
		removeDesignerField,
		moveDesignerField,
		saveDesignerSchema,
		checkDraftOnServer
	} = fp.designer;
</script>

{#if s.selectedForm}
	<FormDesignerWorkspace
		form={s.selectedForm}
		fieldDrafts={s.designerFieldDrafts}
		{fieldTypeOptions}
		selectedIndex={s.selectedDesignerFieldIndex}
		selectedFieldUsage={s.selectedDesignerFieldUsage}
		selectedFieldDependencies={s.selectedDesignerFieldDependencies}
		hiddenFieldKeys={s.memberHiddenFieldKeys}
		lockedFieldKeys={s.memberLockedFieldKeys}
		usageLoading={s.designerUsageLoading}
		saving={s.designerSaving}
		schemaIssues={s.designerSchemaIssues}
		checkContext={s.checkContext}
		onCheckOnServer={checkDraftOnServer}
		onSelectField={(index) => (s.selectedDesignerFieldIndex = index)}
		onAppendField={addDesignerField}
		onUpdateField={updateDesignerField}
		onDuplicateField={duplicateDesignerField}
		onRemoveField={removeDesignerField}
		onMoveField={moveDesignerField}
		onSave={saveDesignerSchema}
	/>
{/if}
