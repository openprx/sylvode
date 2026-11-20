<svelte:options runes />

<script lang="ts">
	import { TableProperties, Plus, Pencil, Trash2 } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { childRelationKeyForField } from '#lib/forms/record-draft-values.js';
	import { displayValue } from '#lib/forms/record-format.js';
	import { t } from 'svelte-i18n';
	import RecordChildEditor from '#lib/components/forms/page/RecordChildEditor.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		parentChildRelationFields,
		startAddChildRecord,
		childRecordsForRelationField,
		childGridFieldsForRelationField,
		childFormForRelationField,
		startEditChildRecord,
		deleteChildRecord,
		childRecordFields
	} = fp.recordDetail;
</script>

<div class="mb-5">
	<div class="mb-3 flex flex-wrap items-center justify-between gap-2">
		<div class="flex items-center gap-2">
			<TableProperties class="h-4 w-4 text-fg-muted" aria-hidden="true" />

			<h4 class="text-sm font-semibold text-fg">
				{$t('forms.childRecords')}
			</h4>
		</div>

		<div class="flex flex-wrap items-center gap-2">
			{#each parentChildRelationFields() as relationField}
				<Button size="sm" variant="secondary" onclick={() => startAddChildRecord(relationField)}>
					<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
					{$t('forms.addChildRecord')} · {fieldLabel(relationField)}
				</Button>
			{/each}
		</div>
	</div>

	{#if s.childEditor}
		<RecordChildEditor />
	{/if}

	{#if parentChildRelationFields().length > 0}
		<div class="space-y-3" data-child-table-grids>
			{#each parentChildRelationFields() as relationField}
				{@const childRecords = childRecordsForRelationField(relationField)}
				{@const childFields = childGridFieldsForRelationField(relationField)}
				{@const childForm = childFormForRelationField(relationField)}

				<section
					class="overflow-hidden rounded-card border border-line"
					data-child-table-grid={relationField.key}
					data-child-table-relation-key={childRelationKeyForField(relationField)}
					data-child-table-child-form={childForm?.key}
				>
					<div
						class="flex flex-wrap items-center justify-between gap-2 border-b border-line bg-nav px-3 py-2"
					>
						<div class="min-w-0">
							<div
								class="truncate text-xs font-semibold uppercase text-fg-muted"
								data-child-table-grid-title={relationField.key}
							>
								{fieldLabel(relationField)}
							</div>

							<div
								class="mt-0.5 text-[11px] text-fg-muted"
								data-child-table-grid-meta={relationField.key}
							>
								{childForm?.name ?? childRelationKeyForField(relationField)} · {childRelationKeyForField(
									relationField
								)}
							</div>
						</div>

						<span
							class="rounded bg-raised px-2 py-0.5 text-[11px] font-medium text-fg-muted ring-1 ring-line"
							data-child-table-grid-count={`${relationField.key}:${childRecords.length}`}
							>{$t('forms.groupRecordCount', {
								values: { count: childRecords.length }
							})}</span
						>
					</div>

					{#if childRecords.length > 0}
						<div class="overflow-x-auto">
							<table
								class="min-w-full divide-y divide-line text-xs"
								data-child-table-grid-table={relationField.key}
							>
								<thead class="bg-nav">
									<tr>
										<th class="min-w-40 px-3 py-2 text-left font-medium text-fg-muted"
											>{$t('forms.recordTitle')}</th
										>

										{#each childFields as childField}
											<th
												class="min-w-32 px-3 py-2 text-left font-medium text-fg-muted"
												data-child-table-grid-column={`${relationField.key}:${childField.key}`}
												>{fieldLabel(childField)}</th
											>
										{/each}

										<th class="w-20 px-3 py-2 text-right font-medium text-fg-muted"
											>{$t('common.actions')}</th
										>
									</tr>
								</thead>

								<tbody class="divide-y divide-line">
									{#each childRecords as child}
										<tr data-child-table-grid-row={`${relationField.key}:${child.record.id}`}>
											<td class="px-3 py-2 font-medium text-fg">{child.record.title}</td>

											{#each childFields as childField}
												<td
													class="max-w-56 px-3 py-2 text-fg"
													data-child-table-grid-cell={`${child.record.id}:${childField.key}`}
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
														data-child-table-edit-row={`${relationField.key}:${child.record.id}`}
														><Pencil class="h-4 w-4" aria-hidden="true" /></button
													>

													<button
														type="button"
														class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
														disabled={s.deletingChildRecordId === child.record.id}
														onclick={() => deleteChildRecord(child)}
														aria-label={$t('forms.deleteChildRecord')}
														data-child-table-delete-row={`${relationField.key}:${child.record.id}`}
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
							data-child-table-grid-empty={relationField.key}
						>
							{$t('forms.noChildRecords')}
						</p>
					{/if}
				</section>
			{/each}
		</div>
	{:else if s.selectedRecordChildren.length > 0}
		<div class="overflow-hidden rounded-control border border-line">
			<table class="min-w-full divide-y divide-line text-xs">
				<thead class="bg-nav">
					<tr>
						<th class="px-3 py-2 text-left font-medium text-fg-muted">{$t('forms.recordTitle')}</th>

						<th class="px-3 py-2 text-left font-medium text-fg-muted">{$t('forms.relationKey')}</th>

						<th class="px-3 py-2 text-left font-medium text-fg-muted">{$t('forms.fields')}</th>

						<th class="px-3 py-2 text-right font-medium text-fg-muted">{$t('common.actions')}</th>
					</tr>
				</thead>

				<tbody class="divide-y divide-line">
					{#each s.selectedRecordChildren as child}
						<tr>
							<td class="px-3 py-2 font-medium text-fg">{child.record.title}</td>

							<td class="px-3 py-2 font-mono text-fg-muted">{child.relation_key}</td>

							<td class="px-3 py-2 text-fg">
								{#each childRecordFields(child).slice(0, 4) as childField, index}
									{#if index > 0}
										<span class="mx-1 text-fg-muted">/</span>
									{/if}

									<span
										>{fieldLabel(childField)}: {displayValue(
											child.record.values[childField.key]
										)}</span
									>
								{/each}
							</td>

							<td class="px-3 py-2 text-right">
								<div class="flex justify-end gap-1">
									<button
										type="button"
										class="inline-flex items-center rounded-control p-1.5 text-fg hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
										onclick={() => startEditChildRecord(child)}
										aria-label={$t('forms.editChildRecord')}
										><Pencil class="h-4 w-4" aria-hidden="true" /></button
									>

									<button
										type="button"
										class="inline-flex items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
										disabled={s.deletingChildRecordId === child.record.id}
										onclick={() => deleteChildRecord(child)}
										aria-label={$t('forms.deleteChildRecord')}
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
		<p class="text-sm text-fg-muted">
			{$t('forms.noChildRecords')}
		</p>
	{/if}
</div>
