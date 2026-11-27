<svelte:options runes />

<script lang="ts">
	import { Plus, RefreshCw, Trash2 } from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import { fieldTypeOptions } from '#lib/forms/page-constants.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';
	import { locale } from 'svelte-i18n';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { loadTemplatePackages } = fp.loaders;
	const { handleCreateForm, installTemplatePackage, addFieldDraft, removeFieldDraft } =
		fp.createForm;

	function packageSummary(
		summary: Record<string, string> | null,
		language: string | null | undefined
	) {
		if (!summary) return '';

		return summary[(language ?? 'en').slice(0, 2)] ?? summary.en ?? '';
	}
	const { buildSchemaFromDrafts } = fp.designer;
</script>

<section class="rounded-card border border-line bg-raised p-4">
	<div class="mb-4 flex items-start justify-between gap-4">
		<div>
			<h2 class="text-base font-semibold text-fg">
				{$t('forms.defineType')}
			</h2>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.defineTypeDescription')}
			</p>
		</div>
	</div>

	<div class="mb-5 rounded-card border border-line bg-nav p-3">
		<div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
			<div>
				<h3 class="text-sm font-semibold text-fg">
					{$t('forms.templateLibrary')}
				</h3>

				<p class="text-xs text-fg-muted">
					{$t('forms.templateLibraryDescription')}
				</p>
			</div>

			<Button
				size="sm"
				variant="secondary"
				loading={s.templatesLoading}
				onclick={loadTemplatePackages}
			>
				<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.refreshTemplates')}
			</Button>
		</div>

		{#if s.templatePackages.length > 0}
			<div class="mt-3 grid gap-2 lg:grid-cols-3">
				{#each s.templatePackages as template (template.id)}
					<div class="rounded-card border border-line bg-raised p-3">
						<div class="min-w-0">
							<h4 class="truncate font-mono text-sm font-semibold text-fg">
								{template.package_key}
							</h4>

							<p class="mt-1 line-clamp-3 text-xs text-fg-muted">
								{packageSummary(template.summary, $locale)}
							</p>

							<p class="mt-2 font-mono text-[11px] text-fg-muted">{template.version}</p>
						</div>

						<div class="mt-3 flex flex-wrap justify-end gap-2">
							<Button
								variant="primary"
								size="sm"
								disabled={template.custom_code}
								loading={s.installingTemplateKey === template.package_key}
								onclick={() => installTemplatePackage(template)}
							>
								<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
								{$t('forms.installTemplate')}
							</Button>
						</div>
					</div>
				{/each}
			</div>
		{:else}
			<p class="mt-3 text-sm text-fg-muted">
				{s.templatesLoading ? $t('common.loading') : $t('forms.noTemplates')}
			</p>
		{/if}
	</div>

	<div class="grid gap-3 md:grid-cols-4">
		<label class="space-y-1">
			<span class="text-sm font-medium text-fg">{$t('forms.key')}</span>

			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.formDraft.key}
			/>
		</label>

		<label class="space-y-1 md:col-span-2">
			<span class="text-sm font-medium text-fg">{$t('forms.name')}</span>

			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.formDraft.name}
			/>
		</label>

		<label class="space-y-1">
			<span class="text-sm font-medium text-fg">{$t('forms.titleTemplate')}</span>

			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.formDraft.title_template}
			/>
		</label>

		<label class="space-y-1 md:col-span-4">
			<span class="text-sm font-medium text-fg">{$t('forms.description')}</span>

			<input
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				bind:value={s.formDraft.description}
			/>
		</label>
	</div>

	<div class="mt-5 rounded-card border border-line">
		<div
			class="flex flex-col gap-2 border-b border-line px-3 py-3 sm:flex-row sm:items-center sm:justify-between"
		>
			<div>
				<h3 class="text-sm font-semibold text-fg">
					{$t('forms.fieldDesigner')}
				</h3>

				<p class="text-xs text-fg-muted">
					{$t('forms.fieldDesignerDescription')}
				</p>
			</div>

			<button
				type="button"
				class="rounded-control border border-line px-3 py-1.5 text-sm text-fg hover:bg-surface"
				onclick={addFieldDraft}>{$t('forms.addField')}</button
			>
		</div>

		<div class="divide-y divide-line">
			{#each s.fieldDrafts as field, index (index)}
				<div class="grid gap-3 p-3 md:grid-cols-[1fr_1fr_160px_90px_44px]">
					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldKey')}</span>

						<input
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							placeholder="customer_name"
							bind:value={field.key}
						/>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldLabel')}</span>

						<input
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							placeholder={$t('forms.fieldLabelPlaceholder')}
							bind:value={field.label}
						/>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium text-fg-muted">{$t('forms.fieldType')}</span>

						<select
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							bind:value={field.type}
						>
							{#each fieldTypeOptions as type, idx (idx)}
								<option value={type}>{$t(`forms.fieldTypes.${type}`)}</option>
							{/each}
						</select>
					</label>

					<label class="flex items-end gap-2 pb-2 text-sm text-fg">
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-input accent-primary"
							bind:checked={field.required}
						/>

						{$t('forms.required')}
					</label>

					<div class="flex items-end justify-end">
						<button
							type="button"
							class="inline-flex h-9 w-9 items-center justify-center rounded-control text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
							disabled={s.fieldDrafts.length === 1}
							onclick={() => removeFieldDraft(index)}
							aria-label={$t('forms.removeField')}
							><Trash2 class="h-4 w-4" aria-hidden="true" /></button
						>
					</div>

					{#if field.type === 'single_select' || field.type === 'multi_select'}
						<label class="space-y-1 md:col-span-5">
							<span class="text-xs font-medium text-fg-muted">{$t('forms.optionList')}</span>

							<textarea
								class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								placeholder={$t('forms.optionListPlaceholder')}
								bind:value={field.optionsText}></textarea>
						</label>
					{/if}

					{#if field.type === 'amount'}
						<div class="grid gap-3 md:col-span-5 md:grid-cols-[160px_120px]">
							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.currency')}</span>

								<input
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm uppercase text-fg"
									maxlength="3"
									bind:value={field.currency}
								/>
							</label>

							<label class="space-y-1">
								<span class="text-xs font-medium text-fg-muted">{$t('forms.scale')}</span>

								<input
									type="number"
									min="0"
									max="8"
									class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
									bind:value={field.scale}
								/>
							</label>
						</div>
					{/if}
				</div>
			{/each}
		</div>
	</div>

	<div class="mt-4 rounded-card border border-line">
		<button
			type="button"
			class="flex w-full items-center justify-between px-3 py-2 text-left text-sm font-medium text-fg hover:bg-surface"
			onclick={() => {
				if (!s.showAdvancedSchema) buildSchemaFromDrafts();

				s.showAdvancedSchema = !s.showAdvancedSchema;
			}}
		>
			<span>{$t('forms.advancedSchema')}</span>
			<span class="text-xs text-fg-muted"
				>{s.showAdvancedSchema ? $t('forms.hideAdvanced') : $t('forms.showAdvanced')}</span
			>
		</button>

		{#if s.showAdvancedSchema}
			<label class="block space-y-1 border-t border-line p-3">
				<span class="text-sm font-medium text-fg">{$t('forms.schemaJson')}</span>

				<textarea
					class="h-56 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
					bind:value={s.formDraft.schemaText}></textarea>
			</label>
		{/if}
	</div>

	<div class="mt-4 flex justify-end">
		<Button variant="primary" loading={s.formSubmitting} onclick={handleCreateForm}
			>{$t('forms.createForm')}</Button
		>
	</div>
</section>
