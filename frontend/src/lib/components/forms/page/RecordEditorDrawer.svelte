<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { FileDown, FileText, FileUp, Pencil, Plus, X } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import FormsSessionNotice from '#lib/components/forms/page/FormsSessionNotice.svelte';
	import {
		attachmentAcceptAttr,
		attachmentIsPreviewableImage,
		attachmentThumbnailUrl,
		signatureImageValue
	} from '#lib/forms/attachment-files.js';
	import { fieldHiddenByCondition, fieldReadOnlyByCondition } from '#lib/forms/field-conditions.js';
	import {
		fieldLabel,
		fieldOptions,
		fieldPlaceholder,
		fieldType,
		optionArchived,
		optionDescription,
		optionDisabled,
		optionDisplayLabel,
		optionGroup,
		recordSelectOptions
	} from '#lib/forms/field-options.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const recordEditorAction = fp.deps.recordEditorAction;
	const {
		handleSaveRecord,
		closeRecordEditorDrawer,
		relationTargetOptions,
		relationDraftValue,
		setDraftValue,
		isDraftOptionSelected,
		toggleDraftOption,
		draftString,
		optionDisabledForSingleDraft,
		optionDisabledForMultiDraft
	} = fp.records;
	const {
		attachmentUrl,
		attachmentPreviewUrl,
		fieldAttachments,
		attachmentSignedUrl,
		createAttachmentForField,
		createAttachmentFromLocalFile,
		removeAttachment,
		startSignatureStroke,
		moveSignatureStroke,
		endSignatureStroke,
		clearSignatureCanvas
	} = fp.attachments;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
</script>

{#if s.selectedForm}
	<div
		use:recordEditorAction
		class="pointer-events-auto absolute inset-0 z-30 flex justify-end bg-scrim"
		data-record-editor-drawer
		role="dialog"
		aria-modal="false"
		aria-labelledby="record-editor-title"
	>
		<button
			type="button"
			class="absolute inset-0 cursor-default"
			aria-hidden="true"
			tabindex="-1"
			onclick={closeRecordEditorDrawer}
		></button>

		<section
			bind:this={s.recordEditorPanel}
			class="relative z-10 flex h-full w-full max-w-3xl flex-col overflow-y-auto bg-overlay p-4 shadow-sheet"
		>
			<div class="mb-3 flex items-center justify-between gap-3">
				<div>
					<h3 id="record-editor-title" class="text-base font-semibold text-fg">
						{s.editingRecordId ? $t('forms.editRecord') : $t('forms.newRecord')}
					</h3>

					<span class="text-xs text-fg-muted">{$t('forms.decimalHint')}</span>
				</div>

				<Button size="sm" variant="secondary" onclick={closeRecordEditorDrawer}>
					<X class="mr-2 h-4 w-4" aria-hidden="true" />
					{$t('common.cancel')}
				</Button>
			</div>

			<FormsSessionNotice />
			<div class="grid gap-3 md:grid-cols-2">
				<label class="space-y-1 md:col-span-2">
					<span class="text-sm font-medium text-fg">{$t('forms.titleOverride')}</span>

					<input
						class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						bind:value={s.recordTitle}
						placeholder={s.selectedForm.title_template}
					/>
				</label>

				{#each s.fields as field}
					<label
						class="space-y-1 {fieldType(field) === 'textarea' ||
						fieldType(field) === 'rich_text' ||
						fieldType(field) === 'signature' ||
						fieldType(field) === 'child_table'
							? 'md:col-span-2'
							: ''} {fieldHiddenByCondition(field, s.recordDraft) ? 'hidden' : ''}"
						data-record-edit-field={field.key}
						data-conditional-hidden={fieldHiddenByCondition(field, s.recordDraft)
							? field.key
							: undefined}
						data-conditional-readonly={fieldReadOnlyByCondition(field, s.recordDraft)
							? field.key
							: undefined}
					>
						<span class="flex flex-wrap items-center gap-2 text-sm font-medium text-fg">
							<span>{fieldLabel(field)}</span>

							{#if field.required}
								<span class="text-danger">*</span>
							{/if}

							{#if memberFieldHidden(field)}
								<span
									class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[11px] font-medium uppercase text-warning"
									data-record-edit-permission-hidden={field.key}
									>{$t('forms.fieldHiddenBadge')}</span
								>
							{/if}

							{#if memberFieldLocked(field)}
								<span
									class="rounded-control bg-surface px-1.5 py-0.5 text-[11px] font-medium uppercase text-fg-muted"
									data-record-edit-permission-locked={field.key}
									>{$t('forms.fieldLockedBadge')}</span
								>
							{/if}
						</span>

						{#if fieldType(field) === 'boolean'}
							<input
								type="checkbox"
								checked={Boolean(s.recordDraft[field.key])}
								disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
								onchange={(event) =>
									setDraftValue(
										field.key,
										event.currentTarget instanceof HTMLInputElement
											? event.currentTarget.checked
											: false
									)}
								class="h-5 w-5 rounded border-input accent-primary"
							/>
						{:else if fieldType(field) === 'single_select'}
							<select
								value={draftString(field.key)}
								disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
								onchange={(event) => setDraftValue(field.key, selectValue(event))}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							>
								<option value="">{$t('forms.selectPlaceholder')}</option>

								{#each recordSelectOptions(field, s.recordDraft) as option}
									<option
										value={option}
										disabled={optionDisabledForSingleDraft(field, option)}
										data-option-disabled={optionDisabled(field, option) ? option : undefined}
										data-option-archived={optionArchived(field, option) ? option : undefined}
										data-option-description={optionDescription(field, option) || undefined}
										data-option-group={optionGroup(field, option) || undefined}
										>{optionDisplayLabel(field, option)}</option
									>
								{/each}
							</select>
						{:else if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
							<div class="grid gap-2 rounded-control border border-input bg-field p-2 text-sm">
								{#each recordSelectOptions(field, s.recordDraft) as option}
									<label
										class="flex items-start gap-2 text-fg"
										data-option-description={optionDescription(field, option) || undefined}
										data-option-group={optionGroup(field, option) || undefined}
									>
										<input
											type="checkbox"
											class="h-4 w-4 rounded border-input accent-primary"
											checked={isDraftOptionSelected(field.key, option)}
											disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
												optionDisabledForMultiDraft(field, option)}
											data-option-disabled={optionDisabled(field, option) ? option : undefined}
											data-option-archived={optionArchived(field, option) ? option : undefined}
											onchange={(event) =>
												toggleDraftOption(
													field.key,
													option,
													event.currentTarget instanceof HTMLInputElement
														? event.currentTarget.checked
														: false
												)}
										/>

										<span class="min-w-0">
											<span class="block">{optionDisplayLabel(field, option)}</span>

											{#if optionDescription(field, option)}
												<span class="block text-xs text-fg-muted"
													>{optionDescription(field, option)}</span
												>
											{/if}
										</span>
									</label>
								{/each}
							</div>
						{:else if fieldType(field) === 'relation' && field.relation?.form_key}
							<select
								value={relationDraftValue(field)}
								disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
									s.relationTargetsLoading}
								onchange={(event) => setDraftValue(field.key, selectValue(event))}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							>
								<option value="">
									{s.relationTargetsLoading
										? $t('forms.loadingRelationTargets')
										: $t('forms.selectRelationTarget')}
								</option>

								{#each relationTargetOptions(field) as target}
									<option value={target.record_id}>{target.display || target.title}</option>
								{/each}
							</select>
						{:else if fieldType(field) === 'child_table'}
							<div
								class="rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg-muted"
								data-child-table-field={field.key}
							>
								{$t('forms.childRecords')}
							</div>
						{:else if fieldType(field) === 'member'}
							<select
								value={draftString(field.key)}
								disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
									s.workspaceMembersLoading}
								onchange={(event) => setDraftValue(field.key, selectValue(event))}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							>
								<option value="">
									{s.workspaceMembersLoading
										? $t('forms.loadingMembers')
										: $t('forms.selectMemberPlaceholder')}
								</option>

								{#each s.workspaceMembers as member (member.user_id)}
									<option value={member.user_id}
										>{member.name || member.email || member.user_id}</option
									>
								{/each}
							</select>
						{:else if ['attachment', 'image'].includes(fieldType(field))}
							<div class="space-y-2" data-attachment-field={field.key}>
								<div class="flex gap-2">
									<input
										type="url"
										class="min-w-0 flex-1 rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
										value={attachmentUrl(field)}
										placeholder={fieldPlaceholder(field) || $t('forms.attachmentUrlPlaceholder')}
										readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
										oninput={(event) => setDraftValue(field.key, inputValue(event))}
										data-attachment-url-input={field.key}
									/>

									<Button
										size="sm"
										variant="secondary"
										disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
											!s.editingRecordId ||
											!attachmentUrl(field)}
										loading={s.attachmentSubmittingField === field.key}
										onclick={() => createAttachmentForField(field)}
									>
										<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
										{$t('forms.attachmentLink')}
									</Button>
								</div>

								<div class="flex flex-wrap items-center gap-2">
									<label
										class="inline-flex cursor-pointer items-center rounded-control border border-input px-3 py-1.5 text-sm font-medium text-fg hover:bg-surface"
									>
										<FileUp class="mr-2 h-4 w-4" aria-hidden="true" />
										{$t('forms.attachmentChooseFile')}

										<input
											type="file"
											class="sr-only"
											accept={attachmentAcceptAttr(field)}
											disabled={fieldReadOnlyByCondition(field, s.recordDraft) ||
												!s.editingRecordId}
											onchange={(event) => createAttachmentFromLocalFile(field, event)}
											data-attachment-file-input={field.key}
										/>
									</label>

									{#if field.attachment?.max_size_mb}
										<span class="text-xs text-fg-muted"
											>{$t('forms.attachmentMaxSizeLabel', {
												values: { size: field.attachment.max_size_mb }
											})}</span
										>
									{/if}
								</div>

								{#if !s.editingRecordId}
									<p class="text-xs text-fg-muted">
										{$t('forms.attachmentSaveRecordFirst')}
									</p>
								{/if}

								{#if attachmentPreviewUrl(field) && fieldType(field) === 'image'}
									<img
										src={attachmentPreviewUrl(field)}
										alt={fieldLabel(field)}
										class="h-24 w-full rounded-card border border-line object-cover"
										data-attachment-preview={field.key}
									/>
								{/if}

								{#if fieldAttachments(field.key).length > 0}
									<div class="space-y-2">
										{#each fieldAttachments(field.key) as attachment}
											<div
												class="flex flex-wrap items-center gap-2 rounded-card border border-line bg-nav px-3 py-2 text-xs"
												data-attachment-item={attachment.id}
											>
												{#if attachment.url && attachmentIsPreviewableImage(attachment)}
													<img
														src={attachmentThumbnailUrl(attachment)}
														alt={attachment.file_name}
														class="h-12 w-12 shrink-0 rounded border border-line object-cover"
														data-attachment-thumbnail={attachment.id}
													/>
												{:else}
													<FileText class="h-4 w-4 shrink-0 text-fg-muted" aria-hidden="true" />
												{/if}

												<div class="min-w-0 flex-1">
													<div class="truncate font-medium text-fg">
														{attachment.file_name}
													</div>

													<div class="truncate font-mono text-[11px] text-fg-muted">
														{attachment.content_type || attachment.storage_key}
													</div>
												</div>

												{#if attachmentSignedUrl(attachment)}
													<a
														href={attachmentSignedUrl(attachment) || undefined}
														target="_blank"
														rel="noreferrer"
														class="inline-flex items-center rounded-control px-2 py-1 text-xs font-medium text-fg underline hover:bg-surface"
														data-attachment-open={attachment.id}
														data-attachment-signed-url={attachmentSignedUrl(attachment)}
														>{$t('forms.attachmentOpen')}</a
													>

													<a
														href={attachmentSignedUrl(attachment) || undefined}
														download={attachment.file_name}
														class="inline-flex items-center rounded-control px-2 py-1 text-xs font-medium text-fg hover:bg-surface"
														data-attachment-download={attachment.id}
														data-attachment-signed-download={attachmentSignedUrl(attachment)}
													>
														<FileDown class="mr-1 h-3.5 w-3.5" aria-hidden="true" />
														{$t('forms.attachmentDownload')}
													</a>
												{/if}

												<Button
													size="sm"
													variant="danger"
													disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
													loading={s.deletingAttachmentId === attachment.id}
													onclick={() => removeAttachment(field, attachment)}
													>{$t('forms.attachmentRemove')}</Button
												>
											</div>
										{/each}
									</div>
								{/if}
							</div>
						{:else if fieldType(field) === 'textarea' || fieldType(field) === 'rich_text'}
							<textarea
								class="min-h-24 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={draftString(field.key)}
								placeholder={fieldPlaceholder(field)}
								readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
								maxlength={field.validation?.max_length}
								oninput={(event) => setDraftValue(field.key, inputValue(event))}></textarea>
						{:else if fieldType(field) === 'signature'}
							<div class="space-y-2" data-signature-field={field.key}>
								<canvas
									class="h-28 w-full touch-none rounded-card border border-dashed border-input bg-white"
									data-signature-canvas={field.key}
									aria-label={$t('forms.signatureCanvas')}
									onpointerdown={(event) =>
										startSignatureStroke(
											event,
											field.key,
											'record',
											fieldReadOnlyByCondition(field, s.recordDraft)
										)}
									onpointermove={(event) =>
										moveSignatureStroke(
											event,
											field.key,
											'record',
											fieldReadOnlyByCondition(field, s.recordDraft)
										)}
									onpointerup={(event) =>
										endSignatureStroke(
											event,
											field.key,
											'record',
											fieldReadOnlyByCondition(field, s.recordDraft)
										)}
									onpointercancel={(event) =>
										endSignatureStroke(
											event,
											field.key,
											'record',
											fieldReadOnlyByCondition(field, s.recordDraft)
										)}
								></canvas>

								<div class="flex flex-wrap items-center gap-2">
									<Button
										size="sm"
										variant="secondary"
										disabled={fieldReadOnlyByCondition(field, s.recordDraft)}
										onclick={() =>
											clearSignatureCanvas(
												field.key,
												'record',
												fieldReadOnlyByCondition(field, s.recordDraft)
											)}>{$t('forms.signatureClear')}</Button
									>

									{#if signatureImageValue(s.recordDraft, field.key)}
										<img
											src={signatureImageValue(s.recordDraft, field.key)}
											alt={fieldLabel(field)}
											class="h-10 max-w-48 rounded border border-line bg-raised object-contain"
											data-signature-preview={field.key}
										/>
									{/if}
								</div>

								<textarea
									class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
									value={draftString(field.key)}
									placeholder={fieldPlaceholder(field)}
									readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
									maxlength={field.validation?.max_length}
									oninput={(event) => setDraftValue(field.key, inputValue(event))}
									data-signature-value={field.key}></textarea>
							</div>
						{:else if fieldType(field) === 'autonumber'}
							<input
								type="text"
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg-muted"
								value={draftString(field.key)}
								placeholder={$t('forms.autonumberGeneratedAfterSave')}
								readonly
								data-autonumber-field={field.key}
							/>
						{:else}
							<input
								type={fieldType(field) === 'date'
									? 'date'
									: fieldType(field) === 'datetime'
										? 'datetime-local'
										: fieldType(field) === 'email'
											? 'email'
											: fieldType(field) === 'phone'
												? 'tel'
												: ['amount', 'number', 'integer', 'rating', 'progress'].includes(
															fieldType(field)
													  )
													? 'number'
													: 'text'}
								step={['integer', 'rating', 'progress'].includes(fieldType(field))
									? '1'
									: ['amount', 'number'].includes(fieldType(field))
										? '0.01'
										: undefined}
								class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
								value={draftString(field.key)}
								placeholder={fieldPlaceholder(field)}
								readonly={fieldReadOnlyByCondition(field, s.recordDraft)}
								min={field.validation?.min}
								max={field.validation?.max}
								pattern={field.validation?.pattern}
								maxlength={field.validation?.max_length}
								oninput={(event) => setDraftValue(field.key, inputValue(event))}
							/>
						{/if}

						{#if field.help_text}
							<span class="block text-xs text-fg-muted">{field.help_text}</span>
						{/if}

						{#if fieldType(field) === 'multi_select' && fieldOptions(field).length > 0}
							<span class="block text-xs text-fg-muted"
								>{$t('forms.options', {
									values: { options: fieldOptions(field).join(', ') }
								})}</span
							>
						{/if}
					</label>
				{/each}
			</div>

			<div class="mt-4 flex justify-end">
				<Button loading={s.recordSubmitting} onclick={handleSaveRecord}>
					{#if s.editingRecordId}
						<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.saveRecord')}
					{:else}
						<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
						{$t('forms.addRecord')}
					{/if}
				</Button>
			</div>
		</section>
	</div>
{/if}
