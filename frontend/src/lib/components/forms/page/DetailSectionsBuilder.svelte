<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ArrowDown, ArrowUp, Plus, Trash2 } from '@lucide/svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import { detailSectionColumns as detailLayoutSectionColumns } from '#lib/forms/detail-layout.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		detailLayoutFieldAssignedTo,
		updateDetailLayoutSectionTitle,
		updateDetailLayoutSectionDescription,
		updateDetailLayoutSectionColumns,
		updateDetailLayoutSectionDensity,
		updateDetailLayoutSectionHideEmptyFields,
		updateDetailLayoutSectionCollapsible,
		updateDetailLayoutSectionDefaultCollapsed,
		addDetailLayoutSection,
		removeDetailLayoutSection,
		moveDetailLayoutSection,
		setDetailLayoutSectionField,
		moveDetailLayoutSectionField
	} = fp.detailLayoutEditor;
	const { saveDetailLayoutSections } = fp.detailLayoutSave;
</script>

<section
	class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode === 'sections'
		? ''
		: 'hidden'}"
	data-detail-layout-section-builder={s.selectedForm?.id ?? ''}
>
	<div class="mb-3 flex items-start justify-between gap-3">
		<div>
			<h3 class="text-base font-semibold text-fg">
				{$t('forms.detailLayoutSections')}
			</h3>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.detailLayoutSectionsDescription')}
			</p>
		</div>

		<Button size="sm" variant="secondary" onclick={addDetailLayoutSection}>
			<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.addDetailSection')}
		</Button>
	</div>

	<div class="space-y-3">
		{#each s.detailLayoutSectionDrafts as section, sectionIndex (sectionIndex)}
			<div class="rounded-card border border-line p-3" data-detail-layout-section-row={section.key}>
				<div
					class="mb-3 grid grid-cols-[minmax(0,1fr)_auto] items-end gap-2 md:grid-cols-[minmax(0,1fr)_120px_132px_auto]"
				>
					<label class="min-w-0 space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.detailLayoutSectionTitle')}</span
						>

						<input
							value={section.title}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							data-detail-layout-section-title={section.key}
							oninput={(event) =>
								updateDetailLayoutSectionTitle(
									sectionIndex,
									event.currentTarget instanceof HTMLInputElement ? event.currentTarget.value : ''
								)}
						/>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.detailLayoutSectionColumns')}</span
						>

						<select
							value={String(section.columns)}
							disabled={s.detailLayoutSectionSaving}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							data-detail-layout-section-columns={section.key}
							onchange={(event) =>
								updateDetailLayoutSectionColumns(
									sectionIndex,
									event.currentTarget instanceof HTMLSelectElement
										? detailLayoutSectionColumns(event.currentTarget.value)
										: 2
								)}
						>
							<option value="1">{$t('forms.detailLayoutSectionColumnsOne')}</option>
							<option value="2">{$t('forms.detailLayoutSectionColumnsTwo')}</option>
							<option value="3">{$t('forms.detailLayoutSectionColumnsThree')}</option>
						</select>
					</label>

					<label class="space-y-1">
						<span class="text-xs font-medium uppercase text-fg-muted"
							>{$t('forms.detailLayoutSectionDensity')}</span
						>

						<select
							value={section.density}
							disabled={s.detailLayoutSectionSaving}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							data-detail-layout-section-density={section.key}
							onchange={(event) =>
								updateDetailLayoutSectionDensity(
									sectionIndex,
									event.currentTarget instanceof HTMLSelectElement &&
										event.currentTarget.value === 'compact'
										? 'compact'
										: 'comfortable'
								)}
						>
							<option value="comfortable"
								>{$t('forms.detailLayoutSectionDensityComfortable')}</option
							>
							<option value="compact">{$t('forms.detailLayoutSectionDensityCompact')}</option>
						</select>
					</label>

					<div class="flex items-center gap-1">
						<button
							type="button"
							class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							disabled={sectionIndex === 0 || s.detailLayoutSectionSaving}
							aria-label={$t('forms.moveSectionUp')}
							title={$t('forms.moveSectionUp')}
							data-detail-layout-section-move-up={section.key}
							onclick={() => moveDetailLayoutSection(sectionIndex, -1)}
							><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
						>

						<button
							type="button"
							class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							disabled={sectionIndex === s.detailLayoutSectionDrafts.length - 1 ||
								s.detailLayoutSectionSaving}
							aria-label={$t('forms.moveSectionDown')}
							title={$t('forms.moveSectionDown')}
							data-detail-layout-section-move-down={section.key}
							onclick={() => moveDetailLayoutSection(sectionIndex, 1)}
							><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
						>

						<button
							type="button"
							class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
							disabled={s.detailLayoutSectionDrafts.length <= 1 || s.detailLayoutSectionSaving}
							aria-label={$t('forms.removeDetailSection')}
							title={$t('forms.removeDetailSection')}
							data-detail-layout-section-remove={section.key}
							onclick={() => removeDetailLayoutSection(sectionIndex)}
							><Trash2 class="h-4 w-4" aria-hidden="true" /></button
						>
					</div>
				</div>

				<label class="mb-3 block space-y-1">
					<span class="text-xs font-medium uppercase text-fg-muted"
						>{$t('forms.detailLayoutSectionDescription')}</span
					>

					<textarea
						value={section.description}
						rows="2"
						disabled={s.detailLayoutSectionSaving}
						placeholder={$t('forms.detailLayoutSectionDescriptionPlaceholder')}
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
						data-detail-layout-section-description={section.key}
						oninput={(event) =>
							updateDetailLayoutSectionDescription(
								sectionIndex,
								event.currentTarget instanceof HTMLTextAreaElement ? event.currentTarget.value : ''
							)}></textarea>
				</label>

				<label class="mb-3 flex items-center gap-2 text-sm text-fg">
					<input
						type="checkbox"
						class="h-4 w-4 rounded border-input accent-primary"
						checked={section.hideEmptyFields}
						disabled={s.detailLayoutSectionSaving}
						data-detail-layout-section-hide-empty-fields={section.key}
						onchange={(event) =>
							updateDetailLayoutSectionHideEmptyFields(
								sectionIndex,
								event.currentTarget instanceof HTMLInputElement
									? event.currentTarget.checked
									: false
							)}
					/>

					<span>{$t('forms.detailLayoutSectionHideEmptyFields')}</span>
				</label>

				<div class="mb-3 grid gap-2 sm:grid-cols-2">
					<label class="flex items-center gap-2 text-sm text-fg">
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-input accent-primary"
							checked={section.collapsible}
							disabled={s.detailLayoutSectionSaving}
							data-detail-layout-section-collapsible={section.key}
							onchange={(event) =>
								updateDetailLayoutSectionCollapsible(
									sectionIndex,
									event.currentTarget instanceof HTMLInputElement
										? event.currentTarget.checked
										: false
								)}
						/>

						<span>{$t('forms.detailLayoutSectionCollapsible')}</span>
					</label>

					<label class="flex items-center gap-2 text-sm text-fg">
						<input
							type="checkbox"
							class="h-4 w-4 rounded border-input accent-primary"
							checked={section.defaultCollapsed}
							disabled={s.detailLayoutSectionSaving || !section.collapsible}
							data-detail-layout-section-default-collapsed={section.key}
							onchange={(event) =>
								updateDetailLayoutSectionDefaultCollapsed(
									sectionIndex,
									event.currentTarget instanceof HTMLInputElement
										? event.currentTarget.checked
										: false
								)}
						/>

						<span>{$t('forms.detailLayoutSectionDefaultCollapsed')}</span>
					</label>
				</div>

				<div class="space-y-1.5">
					{#each s.detailVisibleFields as field (field.key)}
						{@const assignedSectionKey = detailLayoutFieldAssignedTo(field.key)}
						{@const fieldIndex = section.fields.indexOf(field.key)}

						<div
							class="grid min-h-9 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-control px-2 py-1 text-sm hover:bg-surface"
							data-detail-layout-section-field={`${section.key}:${field.key}`}
							data-detail-layout-section-field-assigned={assignedSectionKey || 'unassigned'}
						>
							<input
								type="checkbox"
								class="h-4 w-4 rounded border-input accent-primary"
								checked={section.fields.includes(field.key)}
								disabled={s.detailLayoutSectionSaving}
								aria-label={`${$t('forms.detailLayoutField')}: ${fieldLabel(field)} / ${section.title}`}
								onchange={(event) =>
									setDetailLayoutSectionField(
										sectionIndex,
										field.key,
										event.currentTarget instanceof HTMLInputElement
											? event.currentTarget.checked
											: false
									)}
							/>

							<div class="min-w-0">
								<span class="block truncate text-fg">{fieldLabel(field)}</span>

								{#if assignedSectionKey && assignedSectionKey !== section.key}
									<span class="text-xs text-fg-muted">{$t('forms.detailLayoutFieldAssigned')}</span>
								{/if}
							</div>

							<div class="flex items-center gap-1">
								<button
									type="button"
									class="inline-flex h-7 w-7 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
									disabled={fieldIndex <= 0 || s.detailLayoutSectionSaving}
									aria-label={$t('forms.moveFieldUp')}
									title={$t('forms.moveFieldUp')}
									data-detail-layout-field-move-up={`${section.key}:${field.key}`}
									onclick={() => moveDetailLayoutSectionField(sectionIndex, field.key, -1)}
									><ArrowUp class="h-3.5 w-3.5" aria-hidden="true" /></button
								>

								<button
									type="button"
									class="inline-flex h-7 w-7 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
									disabled={fieldIndex < 0 ||
										fieldIndex === section.fields.length - 1 ||
										s.detailLayoutSectionSaving}
									aria-label={$t('forms.moveFieldDown')}
									title={$t('forms.moveFieldDown')}
									data-detail-layout-field-move-down={`${section.key}:${field.key}`}
									onclick={() => moveDetailLayoutSectionField(sectionIndex, field.key, 1)}
									><ArrowDown class="h-3.5 w-3.5" aria-hidden="true" /></button
								>
							</div>
						</div>
					{/each}
				</div>
			</div>
		{/each}
	</div>

	<div class="mt-3 flex justify-end">
		<span data-detail-layout-section-save>
			<Button
				variant="primary"
				size="sm"
				loading={s.detailLayoutSectionSaving}
				onclick={saveDetailLayoutSections}>{$t('forms.saveDetailSections')}</Button
			>
		</span>
	</div>
</section>
