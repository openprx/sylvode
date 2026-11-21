<svelte:options runes />

<script lang="ts">
	import FormDataToolbar from '#lib/components/forms/page/FormDataToolbar.svelte';
	import FormConfigHeader from '#lib/components/forms/page/FormConfigHeader.svelte';
	import PermissionStateSummary from '#lib/components/forms/page/PermissionStateSummary.svelte';
	import FormPermissionsMode from '#lib/components/forms/page/FormPermissionsMode.svelte';
	import FormImportExportMode from '#lib/components/forms/page/FormImportExportMode.svelte';
	import FormDesignMode from '#lib/components/forms/page/FormDesignMode.svelte';
	import DetailLayoutMode from '#lib/components/forms/page/DetailLayoutMode.svelte';
	import RecordListStack from '#lib/components/forms/page/RecordListStack.svelte';
	import RecordDetailAside from '#lib/components/forms/page/RecordDetailAside.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
</script>

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
				<RecordListStack />
			{/if}

			{#if s.activeMode === 'detail'}
				<RecordDetailAside />
			{/if}
		</div>
	{/if}
</section>
