<svelte:options runes />

<script lang="ts">
	import {
		keepDetailSectionOpen,
		detailLayoutSectionGridClass,
		detailLayoutSectionFieldsForRecord,
		detailLayoutSectionFieldClass
	} from '#lib/forms/detail-layout-drafts.js';
	import { fieldLabel, fieldType } from '#lib/forms/field-options.js';
	import {
		recordFieldHiddenByCondition,
		recordFieldReadOnlyByCondition
	} from '#lib/forms/field-conditions.js';
	import { childRelationKeyForField } from '#lib/forms/record-draft-values.js';
	import { displayValue } from '#lib/forms/record-format.js';
	import { Pencil, Trash2 } from '@lucide/svelte';
	import OptionValueBadges from '#lib/components/forms/OptionValueBadges.svelte';
	import { t } from 'svelte-i18n';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { detailLayoutSections } = fp.viewRecords;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const {
		childRecordsForRelationField,
		childGridFieldsForRelationField,
		childFormForRelationField,
		startEditChildRecord,
		deleteChildRecord,
		parentChildRelationFields
	} = fp.recordDetail;
</script>

{#if s.selectedRecord}
	<div class="divide-y divide-line" data-record-detail-layout={s.selectedRecord.id}>
		{#each detailLayoutSections() as section, index (index)}
			<details
				class={section.density === 'compact' ? 'px-4 py-3' : 'px-4 py-4'}
				open={!section.collapsible || !section.defaultCollapsed}
				ontoggle={(event) => keepDetailSectionOpen(event, section.collapsible)}
				data-record-detail-section={`${s.selectedRecord.id}:${section.key}`}
				data-record-detail-section-density={`${s.selectedRecord.id}:${section.key}:${section.density}`}
				data-record-detail-section-collapsible={`${s.selectedRecord.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
				data-record-detail-section-open={`${s.selectedRecord.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
			>
				<summary
					class={section.collapsible ? 'cursor-pointer list-none' : 'cursor-default list-none'}
				>
					<h4 class="mb-3 inline text-xs font-semibold uppercase text-fg-muted">
						{section.title}
					</h4>
				</summary>

				{#if section.description}
					<p
						class="mb-3 text-sm text-fg-muted"
						data-record-detail-section-description={`${s.selectedRecord.id}:${section.key}`}
					>
						{section.description}
					</p>
				{/if}

				<dl
					class={detailLayoutSectionGridClass(section.columns, section.density)}
					data-record-detail-section-columns={`${s.selectedRecord.id}:${section.key}:${section.columns}`}
				>
					{#each detailLayoutSectionFieldsForRecord(section, s.selectedRecord) as field (field.key)}
						<div
							class={detailLayoutSectionFieldClass(field, section.columns)}
							data-record-detail-field={`${s.selectedRecord.id}:${section.key}:${field.key}`}
						>
							<dt
								class="flex flex-wrap items-center gap-1.5 text-xs font-medium uppercase text-fg-muted"
							>
								<span class="min-w-0 truncate">{fieldLabel(field)}</span>

								{#if memberFieldHidden(field)}
									<span
										class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium text-warning"
										data-record-detail-permission-hidden={field.key}
										>{$t('forms.fieldHiddenBadge')}</span
									>
								{/if}

								{#if memberFieldLocked(field)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-record-detail-permission-locked={field.key}
										>{$t('forms.fieldLockedBadge')}</span
									>
								{/if}

								{#if recordFieldHiddenByCondition(field, s.selectedRecord)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-record-detail-conditional-hidden={field.key}
										>{$t('forms.fieldConditionHiddenBadge')}</span
									>
								{/if}

								{#if recordFieldReadOnlyByCondition(field, s.selectedRecord)}
									<span
										class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium text-fg-muted"
										data-record-detail-conditional-readonly={field.key}
										>{$t('forms.fieldConditionReadOnlyBadge')}</span
									>
								{/if}
							</dt>

							<dd class="mt-1 break-words text-sm text-fg">
								{#if fieldType(field) === 'child_table'}
									{@const childRecords = childRecordsForRelationField(field)}
									{@const childFields = childGridFieldsForRelationField(field)}
									{@const childForm = childFormForRelationField(field)}

									<div
										class="overflow-hidden rounded-control border border-line"
										data-record-detail-child-table-widget={`${s.selectedRecord.id}:${section.key}:${field.key}`}
										data-record-detail-child-table-relation-key={childRelationKeyForField(field)}
										data-record-detail-child-table-child-form={childForm?.key}
									>
										<div
											class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
										>
											<div class="min-w-0">
												<div
													class="truncate text-xs font-semibold uppercase text-fg-muted"
													data-record-detail-child-table-title={`${s.selectedRecord.id}:${field.key}`}
												>
													{fieldLabel(field)}
												</div>

												<div
													class="mt-0.5 text-[11px] text-fg-muted"
													data-record-detail-child-table-meta={`${s.selectedRecord.id}:${field.key}`}
												>
													{childForm?.name ?? childRelationKeyForField(field)} · {childRelationKeyForField(
														field
													)}
												</div>
											</div>

											<span
												class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
												data-record-detail-child-table-count={`${field.key}:${childRecords.length}`}
												>{$t('forms.groupRecordCount', {
													values: { count: childRecords.length }
												})}</span
											>
										</div>

										{#if childRecords.length > 0}
											<div class="overflow-x-auto">
												<table
													class="min-w-full divide-y divide-line text-xs"
													data-record-detail-child-table-grid={field.key}
												>
													<thead class="bg-nav">
														<tr>
															<th class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
																>{$t('forms.recordTitle')}</th
															>

															{#each childFields as childField, index (index)}
																<th
																	class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
																	data-record-detail-child-table-column={`${field.key}:${childField.key}`}
																	>{fieldLabel(childField)}</th
																>
															{/each}

															<th class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
																>{$t('common.actions')}</th
															>
														</tr>
													</thead>

													<tbody class="divide-y divide-line">
														{#each childRecords as child (child.link_id)}
															<tr
																data-record-detail-child-table-row={`${field.key}:${child.record.id}`}
															>
																<td class="px-3 py-2 font-medium text-fg">{child.record.title}</td>

																{#each childFields as childField, index (index)}
																	<td
																		class="max-w-56 px-3 py-2 text-fg"
																		data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
																		><span class="line-clamp-2 break-words"
																			>{displayValue(child.record.values[childField.key])}</span
																		></td
																	>
																{/each}

																<td class="px-3 py-2 text-right">
																	<div class="flex justify-end gap-1">
																		<button
																			type="button"
																			class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																			onclick={() => startEditChildRecord(child)}
																			aria-label={$t('forms.editChildRecord')}
																			data-record-detail-child-table-edit-row={`${field.key}:${child.record.id}`}
																			><Pencil class="h-4 w-4" aria-hidden="true" /></button
																		>

																		<button
																			type="button"
																			class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																			disabled={s.deletingChildRecordId === child.record.id}
																			onclick={() => deleteChildRecord(child)}
																			aria-label={$t('forms.deleteChildRecord')}
																			data-record-detail-child-table-delete-row={`${field.key}:${child.record.id}`}
																			><Trash2 class="h-4 w-4" aria-hidden="true" /></button
																		>
																	</div>
																</td>
															</tr>
														{/each}
													</tbody>
												</table>
											</div>
										{:else}
											<p
												class="px-3 py-3 text-sm text-fg-muted"
												data-record-detail-child-table-empty={field.key}
											>
												{$t('forms.noChildRecords')}
											</p>
										{/if}
									</div>
								{:else}
									<OptionValueBadges {field} value={s.selectedRecord.values[field.key]} />
								{/if}
							</dd>
						</div>
					{/each}
				</dl>
			</details>
		{/each}

		{#if parentChildRelationFields().length > 0}
			<section class="px-4 py-4" data-record-detail-section={`${s.selectedRecord.id}:child_tables`}>
				<h4 class="mb-3 text-xs font-semibold uppercase text-fg-muted">
					{$t('forms.childRecords')}
				</h4>

				<div class="space-y-3" data-record-detail-child-table-widgets>
					{#each parentChildRelationFields() as relationField, index (index)}
						{@const childRecords = childRecordsForRelationField(relationField)}
						{@const childFields = childGridFieldsForRelationField(relationField)}
						{@const childForm = childFormForRelationField(relationField)}

						<div
							class="overflow-hidden rounded-control border border-line"
							data-record-detail-child-table-widget={`${s.selectedRecord.id}:child_tables:${relationField.key}`}
							data-record-detail-child-table-relation-key={childRelationKeyForField(relationField)}
							data-record-detail-child-table-child-form={childForm?.key}
						>
							<div
								class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
							>
								<div class="min-w-0">
									<div
										class="truncate text-xs font-semibold uppercase text-fg-muted"
										data-record-detail-child-table-title={`${s.selectedRecord.id}:${relationField.key}`}
									>
										{fieldLabel(relationField)}
									</div>

									<div
										class="mt-0.5 text-[11px] text-fg-muted"
										data-record-detail-child-table-meta={`${s.selectedRecord.id}:${relationField.key}`}
									>
										{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(
											relationField
										)}
									</div>
								</div>

								<span
									class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
									data-record-detail-child-table-count={`${relationField.key}:${childRecords.length}`}
									>{$t('forms.groupRecordCount', {
										values: { count: childRecords.length }
									})}</span
								>
							</div>

							{#if childRecords.length > 0}
								<div class="overflow-x-auto">
									<table
										class="min-w-full divide-y divide-line text-xs"
										data-record-detail-child-table-grid={relationField.key}
									>
										<thead class="bg-nav">
											<tr>
												<th class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
													>{$t('forms.recordTitle')}</th
												>

												{#each childFields as childField, index (index)}
													<th
														class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
														data-record-detail-child-table-column={`${relationField.key}:${childField.key}`}
														>{fieldLabel(childField)}</th
													>
												{/each}

												<th class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
													>{$t('common.actions')}</th
												>
											</tr>
										</thead>

										<tbody class="divide-y divide-line">
											{#each childRecords as child (child.link_id)}
												<tr
													data-record-detail-child-table-row={`${relationField.key}:${child.record.id}`}
												>
													<td class="px-3 py-2 font-medium text-fg">{child.record.title}</td>

													{#each childFields as childField, index (index)}
														<td
															class="max-w-56 px-3 py-2 text-fg"
															data-record-detail-child-table-cell={`${child.record.id}:${childField.key}`}
															><span class="line-clamp-2 break-words"
																>{displayValue(child.record.values[childField.key])}</span
															></td
														>
													{/each}

													<td class="px-3 py-2 text-right">
														<div class="flex justify-end gap-1">
															<button
																type="button"
																class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
																onclick={() => startEditChildRecord(child)}
																aria-label={$t('forms.editChildRecord')}
																data-record-detail-child-table-edit-row={`${relationField.key}:${child.record.id}`}
																><Pencil class="h-4 w-4" aria-hidden="true" /></button
															>

															<button
																type="button"
																class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
																disabled={s.deletingChildRecordId === child.record.id}
																onclick={() => deleteChildRecord(child)}
																aria-label={$t('forms.deleteChildRecord')}
																data-record-detail-child-table-delete-row={`${relationField.key}:${child.record.id}`}
																><Trash2 class="h-4 w-4" aria-hidden="true" /></button
															>
														</div>
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								</div>
							{:else}
								<p
									class="px-3 py-3 text-sm text-fg-muted"
									data-record-detail-child-table-empty={relationField.key}
								>
									{$t('forms.noChildRecords')}
								</p>
							{/if}
						</div>
					{/each}
				</div>
			</section>
		{/if}
	</div>
{/if}
