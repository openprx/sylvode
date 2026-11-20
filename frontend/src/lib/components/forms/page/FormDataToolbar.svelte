<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '#lib/components/Button.svelte';
	import FormPluginToolbar from '#lib/components/plugins/FormPluginToolbar.svelte';
	import { FileText, FileUp, Plus, Shield, TableProperties } from '@lucide/svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleFormChange, openRecordCreateDrawer, handleModeChange } = fp.records;
</script>

<section
	class="rounded-card border border-line bg-raised p-4"
	data-form-data-list-toolbar
>
	<div class="flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between">
		<div>
			<div class="flex flex-wrap items-center gap-2">
				<h3 class="text-base font-semibold text-fg">
					{$t('forms.recordsList')}
				</h3>

				<span class="rounded-control bg-surface px-2 py-0.5 text-xs text-fg-muted">
					{#if s.recordsLoading}
						{$t('common.loading')}
					{:else if s.viewRecords.length === s.records.length}
						{$t('forms.rows', { values: { count: s.records.length } })}
					{:else}
						{$t('forms.filteredRows', {
							values: { visible: s.viewRecords.length, total: s.records.length }
						})}
					{/if}
				</span>
			</div>
		</div>

		<div class="flex flex-wrap items-center gap-1.5">
			<label class="inline-flex items-center gap-2 text-xs font-medium text-fg-muted">
				<span>{$t('forms.dataTableSwitcher')}</span>

				<select
					id="form-switcher"
					value={s.selectedFormId}
					onchange={handleFormChange}
					class="h-8 min-w-40 rounded-control border border-input bg-field px-2 text-xs text-fg"
				>
					{#each s.forms as form}
						<option value={form.id}>{form.name}</option>
					{/each}
				</select>
			</label>

			<Button size="sm" onclick={openRecordCreateDrawer}>
				<Plus class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.newRecordAction')}
			</Button>

			<FormPluginToolbar
				workspaceId={s.workspaceId}
				projectId={s.projectId}
				form={s.selectedForm}
			/>

			<button
				type="button"
				class="inline-flex h-8 items-center rounded-control border border-input px-2.5 text-xs font-medium text-fg hover:bg-surface"
				onclick={() => handleModeChange('design')}
			>
				<TableProperties class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.modes.design')}
			</button>

			<button
				type="button"
				class="inline-flex h-8 items-center rounded-control border border-input px-2.5 text-xs font-medium text-fg hover:bg-surface"
				onclick={() => handleModeChange('layout')}
			>
				<FileText class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.modes.layout')}
			</button>

			<button
				type="button"
				class="inline-flex h-8 items-center rounded-control border border-input px-2.5 text-xs font-medium text-fg hover:bg-surface"
				onclick={() => handleModeChange('permissions')}
			>
				<Shield class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.modes.permissions')}
			</button>

			<button
				type="button"
				class="inline-flex h-8 items-center rounded-control border border-input px-2.5 text-xs font-medium text-fg hover:bg-surface"
				onclick={() => handleModeChange('importExport')}
			>
				<FileUp class="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
				{$t('forms.modes.importExport')}
			</button>
		</div>
	</div>
</section>
