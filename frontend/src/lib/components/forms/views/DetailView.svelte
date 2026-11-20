<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { Trash2 } from '@lucide/svelte';
	import OptionValueBadges from '#lib/components/forms/OptionValueBadges.svelte';
	import {
		detailLayoutSectionFieldClass,
		detailLayoutSectionFieldsForRecord,
		detailLayoutSectionGridClass,
		keepDetailSectionOpen
	} from '#lib/forms/detail-layout-drafts.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { formatDate } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleDeleteRecord } = fp.records;
	const { selectRecord } = fp.recordPanel;
	const { detailLayoutSections } = fp.viewRecords;
</script>

<div class="divide-y divide-line" data-view-renderer="detail">
	{#each s.viewRecords as record}
		<article class="p-4 hover:bg-surface">
			<div class="flex items-start justify-between gap-3">
				<button
					type="button"
					class="min-w-0 text-left"
					onclick={() => selectRecord(record)}
					aria-label={`${$t('forms.openRecord')}: ${record.title}`}
				>
					<span class="block truncate font-medium text-fg">{record.title}</span>

					<span class="mt-1 block text-xs text-fg-muted"
						>{$t('forms.updatedAt', {
							values: { date: formatDate(record.updated_at) }
						})}</span
					>
				</button>

				<button
					type="button"
					class="inline-flex shrink-0 items-center rounded-control p-1.5 text-danger hover:bg-danger-soft disabled:bg-surface disabled:text-fg-muted"
					disabled={s.deletingRecordId === record.id}
					onclick={() => handleDeleteRecord(record)}
					aria-label={$t('forms.deleteRecord')}
					><Trash2 class="h-4 w-4" aria-hidden="true" /></button
				>
			</div>

			<div class="mt-3 space-y-3">
				{#each detailLayoutSections() as section}
					<details
						open={!section.collapsible || !section.defaultCollapsed}
						ontoggle={(event) =>
							keepDetailSectionOpen(event, section.collapsible)}
						data-detail-view-section={`${record.id}:${section.key}`}
						data-detail-view-section-density={`${record.id}:${section.key}:${section.density}`}
						data-detail-view-section-collapsible={`${record.id}:${section.key}:${section.collapsible}:${section.defaultCollapsed}`}
						data-detail-view-section-open={`${record.id}:${section.key}:${!section.collapsible || !section.defaultCollapsed}`}
					>
						<summary
							class={section.collapsible
								? 'cursor-pointer list-none'
								: 'cursor-default list-none'}
						>
							<h4
								class="mb-2 inline text-xs font-semibold uppercase text-fg-muted"
							>
								{section.title}
							</h4>
						</summary>

						{#if section.description}
							<p
								class="mb-3 text-sm text-fg-muted"
								data-detail-view-section-description={`${record.id}:${section.key}`}
							>
								{section.description}
							</p>
						{/if}

						<dl
							class={detailLayoutSectionGridClass(
								section.columns,
								section.density
							)}
							data-detail-view-section-columns={`${record.id}:${section.key}:${section.columns}`}
						>
							{#each detailLayoutSectionFieldsForRecord(section, record) as field}
								<div
									class={detailLayoutSectionFieldClass(field, section.columns)}
									data-detail-view-field={`${record.id}:${section.key}:${field.key}`}
								>
									<dt
										class="truncate text-xs font-medium uppercase text-fg-muted"
									>
										{fieldLabel(field)}
									</dt>

									<dd class="mt-1 truncate text-sm text-fg">
										<OptionValueBadges
											{field}
											value={record.values[field.key]}
										/>
									</dd>
								</div>
							{/each}
						</dl>
					</details>
				{/each}
			</div>
		</article>
	{/each}
</div>
