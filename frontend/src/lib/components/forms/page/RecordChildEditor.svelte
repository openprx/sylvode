<svelte:options runes />

<script lang="ts">
	import Button from '#lib/ui/button/Button.svelte';
	import { X, Pencil } from '@lucide/svelte';
	import { fieldInputHints } from '#lib/forms/field-rules.js';
	import { inputValue, selectValue } from '#lib/forms/record-format.js';
	import { fieldHiddenByCondition, fieldReadOnlyByCondition } from '#lib/forms/field-conditions.js';
	import {
		fieldLabel,
		fieldType,
		fieldOptions,
		recordSelectOptions,
		optionDisabled,
		optionArchived,
		optionDescription,
		optionGroup,
		optionDisplayLabel,
		fieldPlaceholder
	} from '#lib/forms/field-options.js';
	import { draftStringFrom } from '#lib/forms/record-draft-values.js';
	import { signatureImageValue } from '#lib/forms/attachment-files.js';
	import { t } from 'svelte-i18n';
	import { directoryName } from '#lib/people/display.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		cancelChildEditor,
		updateChildTitle,
		childFieldsForForm,
		updateChildDraftValue,
		optionDisabledForChildSingleDraft,
		isChildDraftOptionSelected,
		optionDisabledForChildMultiDraft,
		toggleChildDraftOption,
		saveChildRecord
	} = fp.recordDetail;
	const {
		startSignatureStroke,
		moveSignatureStroke,
		endSignatureStroke,
		clearSignatureCanvas,
		setSignatureValue
	} = fp.attachments;
</script>

{#if s.childEditor}
	<div class="mb-3 rounded-card border border-line bg-surface p-3">
		<div class="mb-3 flex items-center justify-between gap-3">
			<h5 class="text-sm font-semibold text-fg">
				{s.childEditor.recordId ? $t('forms.editChildRecord') : $t('forms.addChildRecord')}
			</h5>

			<Button size="sm" variant="secondary" onclick={cancelChildEditor}>
				<X class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('common.cancel')}
			</Button>
		</div>

		<div class="grid gap-3 md:grid-cols-2">
			<label class="space-y-1 text-sm">
				<span class="font-medium text-fg">{$t('forms.titleOverride')}</span>

				<input
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					value={s.childEditor.title}
					oninput={(event) => updateChildTitle(inputValue(event))}
				/>
			</label>

			{#each childFieldsForForm(s.childEditor.childFormId) as childField, index (index)}
				<label
					class="space-y-1 text-sm {fieldHiddenByCondition(childField, s.childEditor.draft)
						? 'hidden'
						: ''}"
					data-child-record-edit-field={childField.key}
					data-conditional-hidden={fieldHiddenByCondition(childField, s.childEditor.draft)
						? childField.key
						: undefined}
					data-conditional-readonly={fieldReadOnlyByCondition(childField, s.childEditor.draft)
						? childField.key
						: undefined}
				>
					<span class="font-medium text-fg">
						{fieldLabel(childField)}

						{#if childField.required}
							<span class="text-danger">*</span>
						{/if}
					</span>

					{#if fieldType(childField) === 'boolean'}
						<div
							class="flex min-h-10 items-center rounded-card border border-input bg-field px-3 py-2"
						>
							<input
								type="checkbox"
								class="h-4 w-4 rounded border-input accent-primary"
								checked={s.childEditor.draft[childField.key] === true}
								disabled={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
								onchange={(event) =>
									updateChildDraftValue(
										childField.key,
										event.currentTarget instanceof HTMLInputElement
											? event.currentTarget.checked
											: false
									)}
							/>
						</div>
					{:else if fieldType(childField) === 'single_select' && fieldOptions(childField).length > 0}
						<select
							value={draftStringFrom(s.childEditor.draft, childField.key)}
							disabled={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
							onchange={(event) => updateChildDraftValue(childField.key, selectValue(event))}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						>
							<option value="">{$t('forms.selectPlaceholder')}</option>

							{#each recordSelectOptions(childField, s.childEditor.draft) as option, index (index)}
								<option
									value={option}
									disabled={optionDisabledForChildSingleDraft(childField, option)}
									data-option-disabled={optionDisabled(childField, option) ? option : undefined}
									data-option-archived={optionArchived(childField, option) ? option : undefined}
									data-option-description={optionDescription(childField, option) || undefined}
									data-option-group={optionGroup(childField, option) || undefined}
									>{optionDisplayLabel(childField, option)}</option
								>
							{/each}
						</select>
					{:else if fieldType(childField) === 'multi_select' && fieldOptions(childField).length > 0}
						<div class="grid gap-2 rounded-control border border-input bg-field p-2 text-sm">
							{#each recordSelectOptions(childField, s.childEditor.draft) as option, index (index)}
								<label
									class="flex items-start gap-2 text-fg"
									data-option-description={optionDescription(childField, option) || undefined}
									data-option-group={optionGroup(childField, option) || undefined}
								>
									<input
										type="checkbox"
										class="h-4 w-4 rounded border-input accent-primary"
										checked={isChildDraftOptionSelected(childField.key, option)}
										disabled={fieldReadOnlyByCondition(childField, s.childEditor.draft) ||
											optionDisabledForChildMultiDraft(childField, option)}
										data-option-disabled={optionDisabled(childField, option) ? option : undefined}
										data-option-archived={optionArchived(childField, option) ? option : undefined}
										onchange={(event) =>
											toggleChildDraftOption(
												childField.key,
												option,
												event.currentTarget instanceof HTMLInputElement
													? event.currentTarget.checked
													: false
											)}
									/>

									<span class="min-w-0">
										<span class="block">{optionDisplayLabel(childField, option)}</span>

										{#if optionDescription(childField, option)}
											<span class="block text-xs text-fg-muted"
												>{optionDescription(childField, option)}</span
											>
										{/if}
									</span>
								</label>
							{/each}
						</div>
					{:else if fieldType(childField) === 'member'}
						<select
							value={draftStringFrom(s.childEditor.draft, childField.key)}
							disabled={fieldReadOnlyByCondition(childField, s.childEditor.draft) ||
								s.workspaceMembersLoading}
							onchange={(event) => updateChildDraftValue(childField.key, selectValue(event))}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
						>
							<option value="">
								{s.workspaceMembersLoading
									? $t('forms.loadingMembers')
									: $t('forms.selectMemberPlaceholder')}
							</option>

							{#each s.workspaceMembers as member (member.user_id)}
								<option value={member.user_id}>{directoryName(member, $t)}</option>
							{/each}
						</select>
					{:else if fieldType(childField) === 'textarea' || fieldType(childField) === 'rich_text'}
						<textarea
							class="min-h-20 w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							value={draftStringFrom(s.childEditor.draft, childField.key)}
							placeholder={fieldPlaceholder(childField)}
							readonly={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
							maxlength={fieldInputHints(childField).maxlength}
							oninput={(event) => updateChildDraftValue(childField.key, inputValue(event))}
						></textarea>
					{:else if fieldType(childField) === 'signature'}
						<div class="space-y-2" data-child-signature-field={childField.key}>
							<canvas
								class="h-24 w-full touch-none rounded-card border border-dashed border-input bg-white"
								data-child-signature-canvas={childField.key}
								aria-label={$t('forms.signatureCanvas')}
								onpointerdown={(event) =>
									startSignatureStroke(
										event,
										childField.key,
										'child',
										s.childEditor ? fieldReadOnlyByCondition(childField, s.childEditor.draft) : true
									)}
								onpointermove={(event) =>
									moveSignatureStroke(
										event,
										childField.key,
										'child',
										s.childEditor ? fieldReadOnlyByCondition(childField, s.childEditor.draft) : true
									)}
								onpointerup={(event) =>
									endSignatureStroke(
										event,
										childField.key,
										'child',
										s.childEditor ? fieldReadOnlyByCondition(childField, s.childEditor.draft) : true
									)}
								onpointercancel={(event) =>
									endSignatureStroke(
										event,
										childField.key,
										'child',
										s.childEditor ? fieldReadOnlyByCondition(childField, s.childEditor.draft) : true
									)}
							></canvas>

							<div class="flex flex-wrap items-center gap-2">
								<Button
									size="sm"
									variant="secondary"
									disabled={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
									onclick={() =>
										clearSignatureCanvas(
											childField.key,
											'child',
											s.childEditor
												? fieldReadOnlyByCondition(childField, s.childEditor.draft)
												: true
										)}>{$t('forms.signatureClear')}</Button
								>

								{#if signatureImageValue(s.childEditor.draft, childField.key)}
									<img
										src={signatureImageValue(s.childEditor.draft, childField.key)}
										alt={fieldLabel(childField)}
										class="h-10 max-w-48 rounded border border-line bg-raised object-contain"
										data-child-signature-preview={childField.key}
									/>
								{/if}
							</div>

							<textarea
								class="min-h-16 w-full rounded-control border border-input bg-field px-3 py-2 font-mono text-xs text-fg"
								value={draftStringFrom(s.childEditor.draft, childField.key)}
								placeholder={fieldPlaceholder(childField)}
								readonly={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
								maxlength={fieldInputHints(childField).maxlength}
								oninput={(event) => {
									if (!setSignatureValue(childField.key, 'child', inputValue(event)))
										event.currentTarget.value = draftStringFrom(
											s.childEditor?.draft ?? {},
											childField.key
										);
								}}
								data-child-signature-value={childField.key}></textarea>
						</div>
					{:else if fieldType(childField) === 'autonumber'}
						<input
							type="text"
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg-muted"
							value={draftStringFrom(s.childEditor.draft, childField.key)}
							placeholder={$t('forms.autonumberGeneratedAfterSave')}
							readonly
							data-child-autonumber-field={childField.key}
						/>
					{:else if fieldType(childField) === 'child_table'}
						<div
							class="rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg-muted"
							data-child-table-field={childField.key}
						>
							{$t('forms.childRecords')}
						</div>
					{:else}
						<input
							type={fieldType(childField) === 'date'
								? 'date'
								: fieldType(childField) === 'datetime'
									? 'datetime-local'
									: fieldType(childField) === 'email'
										? 'email'
										: fieldType(childField) === 'phone'
											? 'tel'
											: ['amount', 'number', 'integer', 'rating', 'progress'].includes(
														fieldType(childField)
												  )
												? 'number'
												: 'text'}
							step={['integer', 'rating', 'progress'].includes(fieldType(childField))
								? '1'
								: ['amount', 'number'].includes(fieldType(childField))
									? '0.01'
									: undefined}
							class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
							value={draftStringFrom(s.childEditor.draft, childField.key)}
							placeholder={fieldPlaceholder(childField)}
							readonly={fieldReadOnlyByCondition(childField, s.childEditor.draft)}
							min={fieldInputHints(childField).min}
							max={fieldInputHints(childField).max}
							maxlength={fieldInputHints(childField).maxlength}
							oninput={(event) => updateChildDraftValue(childField.key, inputValue(event))}
						/>
					{/if}

					{#if childField.help_text}
						<span class="block text-xs text-fg-muted">{childField.help_text}</span>
					{/if}
				</label>
			{/each}
		</div>

		<div class="mt-3 flex justify-end gap-2">
			<Button size="sm" variant="secondary" onclick={cancelChildEditor}
				>{$t('common.cancel')}</Button
			>

			<Button variant="primary" size="sm" loading={s.childSubmitting} onclick={saveChildRecord}>
				<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.saveRecord')}
			</Button>
		</div>
	</div>
{/if}
