<svelte:options runes />

<script lang="ts">
	import { ChevronLeft, ChevronRight, FileDown, FileUp } from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { inputValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		closeImportRecords,
		resetImportPreviewForSourceChange,
		continueImportFromSource,
		continueImportFromMapping,
		exportImportTemplate,
		loadImportFile,
		refreshImportMappingFromText,
		saveImportMappingTemplate,
		restoreImportMappingTemplate,
		applyImportMapping,
		commitImportRecords
	} = fp.importer;
</script>

<Modal
	bind:open={s.showImportRecords}
	title={$t('forms.importRecords')}
	maxWidthClass="max-w-4xl"
	onclose={closeImportRecords}
>
	<div class="space-y-4" data-import-wizard data-import-wizard-step={s.importWizardStep}>
		<div class="grid gap-2 sm:grid-cols-4" aria-label={$t('forms.importWizardSteps')}>
			{#each ['source', 'mapping', 'preview', 'commit'] as step, index (index)}
				{@const current = s.importWizardStep === step}
				{@const complete =
					(step === 'source' && s.importText.trim().length > 0) ||
					(step === 'mapping' && (s.importMappingApplied || s.importMappingHeaders.length === 0)) ||
					(step === 'preview' && Boolean(s.importPreview)) ||
					(step === 'commit' &&
						(s.importPreview?.valid_rows ?? 0) > 0 &&
						s.importPreview?.invalid_rows === 0)}

				<div
					class={current
						? 'rounded-card border border-line bg-surface p-3 text-fg'
						: complete
							? 'rounded-card border border-line bg-success-soft p-3 text-success'
							: 'rounded-card border border-line bg-raised p-3 text-fg-muted'}
					data-import-step={step}
					data-import-step-active={current ? 'true' : undefined}
				>
					<p class="text-xs font-semibold uppercase tracking-wide">
						{$t('forms.importStepCounter', { values: { step: index + 1 } })}
					</p>

					<p class="mt-1 text-sm font-medium">{$t(`forms.importSteps.${step}.title`)}</p>
					<p class="mt-1 text-xs">{$t(`forms.importSteps.${step}.description`)}</p>
				</div>
			{/each}
		</div>

		{#if s.importWizardStep === 'source'}
			<section class="space-y-3" data-import-wizard-panel="source">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<label class="block text-sm font-medium text-fg" for="form-import-text"
						>{$t('forms.importPayload')}</label
					>

					<div class="flex flex-wrap items-center gap-2">
						<label
							class="inline-flex cursor-pointer items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							for="form-import-file"
						>
							<FileUp class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.uploadImportFile')}
						</label>

						<input
							id="form-import-file"
							class="sr-only"
							type="file"
							accept=".csv,.json,text/csv,application/json"
							onchange={loadImportFile}
							data-import-file-upload
							data-import-file-loaded={s.importedImportFileLoaded ? 'true' : undefined}
							data-import-file-name={s.importedImportFileName}
						/>

						<button
							type="button"
							class="inline-flex items-center gap-1 rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={exportImportTemplate}
							disabled={!s.selectedForm || s.visibleFields.length === 0}
							data-import-template-export
							data-import-template-exported={s.exportedImportTemplate ? 'true' : undefined}
						>
							<FileDown class="h-3.5 w-3.5" aria-hidden="true" />
							{$t('forms.downloadImportTemplate')}
						</button>
					</div>
				</div>

				{#if s.importedImportFileName}
					<p class="text-xs text-fg-muted">
						{$t('forms.importFileLoadedName', { values: { name: s.importedImportFileName } })}
					</p>
				{/if}

				<textarea
					id="form-import-text"
					class="min-h-72 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-sm text-fg"
					value={s.importText}
					oninput={(event) => {
						s.importText = inputValue(event);
						s.importedImportFileName = '';
						s.importedImportFileLoaded = false;
						resetImportPreviewForSourceChange();
						refreshImportMappingFromText();
					}}></textarea>

				<p class="text-xs text-fg-muted">{$t('forms.importPayloadHint')}</p>
			</section>
		{/if}

		{#if s.importWizardStep === 'mapping'}
			<section
				class="space-y-3 rounded-card border border-line bg-nav p-3"
				data-import-wizard-panel="mapping"
				data-import-mapping-wizard
				data-import-mapping-applied={s.importMappingApplied ? 'true' : undefined}
				data-import-mapping-template-available={s.importMappingTemplateAvailable
					? 'true'
					: undefined}
				data-import-mapping-template-saved={s.importMappingTemplateSaved ? 'true' : undefined}
				data-import-mapping-template-restored={s.importMappingTemplateRestored ? 'true' : undefined}
			>
				<div class="flex flex-wrap items-center justify-between gap-2">
					<div>
						<h3 class="text-sm font-medium text-fg">
							{$t('forms.importMapping')}
						</h3>

						<p class="mt-1 text-xs text-fg-muted">
							{$t('forms.importMappingHint')}
						</p>
					</div>

					<div class="flex flex-wrap items-center gap-2">
						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							onclick={saveImportMappingTemplate}
							data-import-mapping-template-save>{$t('forms.saveImportMappingTemplate')}</button
						>

						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
							onclick={restoreImportMappingTemplate}
							disabled={!s.importMappingTemplateAvailable}
							data-import-mapping-template-restore
							>{$t('forms.restoreImportMappingTemplate')}</button
						>

						<button
							type="button"
							class="inline-flex items-center rounded-control border border-input bg-raised px-2.5 py-1.5 text-xs font-medium text-fg hover:bg-surface"
							onclick={applyImportMapping}
							data-import-mapping-apply>{$t('forms.applyImportMapping')}</button
						>
					</div>
				</div>

				<div class="grid max-h-[46vh] gap-2 overflow-auto pr-1 sm:grid-cols-2">
					{#each s.importMappingHeaders as header, index (index)}
						<label class="block space-y-1 text-xs" data-import-mapping-row={index}>
							<span class="font-medium text-fg-muted"
								>{header || $t('forms.importUnnamedColumn')}</span
							>

							<select
								class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
								bind:value={s.importMappingSelections[index]}
								onchange={() => {
									s.importMappingApplied = false;
								}}
								data-import-mapping-column={index}
							>
								<option value="__skip">{$t('forms.importMappingSkip')}</option>
								<option value="__title">{$t('forms.recordTitle')}</option>

								{#each s.fields as field (field.key)}
									<option value={field.key}>{fieldLabel(field)}</option>
								{/each}
							</select>

							<span class="font-medium text-fg-muted">{$t('forms.importMappingTransform')}</span>

							<select
								class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
								bind:value={s.importMappingTransforms[index]}
								onchange={() => {
									s.importMappingApplied = false;
								}}
								data-import-mapping-transform={index}
							>
								<option value="none">{$t('forms.importMappingTransformNone')}</option>
								<option value="trim">{$t('forms.importMappingTransformTrim')}</option>
								<option value="uppercase">{$t('forms.importMappingTransformUppercase')}</option>
								<option value="lowercase">{$t('forms.importMappingTransformLowercase')}</option>
								<option value="capitalize">{$t('forms.importMappingTransformCapitalize')}</option>
								<option value="titlecase">{$t('forms.importMappingTransformTitlecase')}</option>
								<option value="normalize_spaces"
									>{$t('forms.importMappingTransformNormalizeSpaces')}</option
								>
								<option value="strip_non_digits"
									>{$t('forms.importMappingTransformStripNonDigits')}</option
								>
								<option value="slugify">{$t('forms.importMappingTransformSlugify')}</option>
							</select>
						</label>
					{/each}
				</div>
			</section>
		{/if}

		{#if s.importWizardStep === 'preview'}
			<section class="space-y-3" data-import-wizard-panel="preview">
				{#if s.importPreview}
					<div class="grid gap-2 sm:grid-cols-3">
						<div class="rounded-card border border-line bg-raised p-3">
							<p class="text-xs text-fg-muted">
								{$t('forms.importTotalRows')}
							</p>

							<p class="mt-1 text-2xl font-semibold text-fg">
								{s.importPreview.total_rows}
							</p>
						</div>

						<div class="rounded-card border border-line bg-success-soft p-3">
							<p class="text-xs text-success">
								{$t('forms.importValidRows')}
							</p>

							<p class="mt-1 text-2xl font-semibold text-success">
								{s.importPreview.valid_rows}
							</p>
						</div>

						<div class="rounded-card border border-line bg-danger-soft p-3">
							<p class="text-xs text-danger">{$t('forms.importInvalidRows')}</p>

							<p class="mt-1 text-2xl font-semibold text-danger">
								{s.importPreview.invalid_rows}
							</p>
						</div>
					</div>

					<div class="rounded-control border border-line">
						<div
							class="flex flex-wrap items-center justify-between gap-2 border-b border-line px-3 py-2 text-sm"
						>
							<span class="font-medium text-fg">{$t('forms.importPreview')}</span>

							<span class="text-fg-muted">
								{$t('forms.importPreviewSummary', {
									values: {
										total: s.importPreview.total_rows,
										valid: s.importPreview.valid_rows,
										invalid: s.importPreview.invalid_rows
									}
								})}
							</span>
						</div>

						<div class="max-h-64 overflow-auto">
							<table class="min-w-full divide-y divide-line text-sm">
								<thead class="bg-nav text-left text-xs font-medium uppercase text-fg-muted">
									<tr>
										<th class="px-3 py-2">{$t('forms.importRow')}</th>
										<th class="px-3 py-2">{$t('common.status')}</th>
										<th class="px-3 py-2">{$t('forms.recordTitle')}</th>
										<th class="px-3 py-2">{$t('forms.importMessage')}</th>
									</tr>
								</thead>

								<tbody class="divide-y divide-line">
									{#each s.importPreview.rows as row, idx (idx)}
										<tr>
											<td class="px-3 py-2 font-mono text-xs text-fg-muted">{row.row_number}</td>

											<td class="px-3 py-2">
												<span
													class={row.valid
														? 'rounded-full bg-success-soft px-2 py-0.5 text-xs font-medium text-success'
														: 'rounded-full bg-danger-soft px-2 py-0.5 text-xs font-medium text-danger'}
													>{row.valid ? $t('forms.importValid') : $t('forms.importInvalid')}</span
												>
											</td>

											<td class="max-w-56 truncate px-3 py-2 text-fg">{row.title ?? '-'}</td>

											<td class="px-3 py-2 text-fg-muted">{row.error ?? '-'}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					</div>
				{/if}
			</section>
		{/if}

		{#if s.importWizardStep === 'commit'}
			<section
				class="rounded-card border border-line bg-success-soft p-4"
				data-import-wizard-panel="commit"
			>
				<p class="text-sm font-semibold text-success">
					{$t('forms.importReadyToCommit')}
				</p>

				<p class="mt-1 text-sm text-success">
					{#if s.importPreview}
						{$t('forms.importReadyToCommitDescription', {
							values: { count: s.importPreview.valid_rows }
						})}
					{/if}
				</p>
			</section>
		{/if}

		<div
			class="flex flex-col-reverse gap-2 border-t border-line pt-3 sm:flex-row sm:items-center sm:justify-between"
		>
			<div class="flex gap-2">
				{#if s.importWizardStep !== 'source'}
					<Button
						variant="secondary"
						onclick={() => {
							s.importWizardStep =
								s.importWizardStep === 'mapping'
									? 'source'
									: s.importWizardStep === 'preview'
										? 'mapping'
										: 'preview';
						}}
					>
						<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('common.back')}
					</Button>
				{/if}
			</div>

			<div class="flex justify-end gap-2">
				<Button variant="secondary" onclick={closeImportRecords}>{$t('common.cancel')}</Button>

				{#if s.importWizardStep === 'source'}
					<Button
						variant="primary"
						disabled={!s.importText.trim()}
						loading={s.importPreviewing}
						onclick={continueImportFromSource}
					>
						{$t('forms.importContinueToMapping')}
						<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
					</Button>
				{:else if s.importWizardStep === 'mapping'}
					<Button
						variant="primary"
						loading={s.importPreviewing}
						onclick={continueImportFromMapping}
					>
						{$t('forms.previewImport')}
						<ChevronRight class="ml-2 h-4 w-4" aria-hidden="true" />
					</Button>
				{:else}
					<Button
						variant="primary"
						loading={s.importingRecords}
						disabled={!s.importPreview ||
							s.importPreview.invalid_rows > 0 ||
							s.importPreview.valid_rows === 0}
						onclick={commitImportRecords}
					>
						<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.commitImport')}
					</Button>
				{/if}
			</div>
		</div>
	</div>
</Modal>
