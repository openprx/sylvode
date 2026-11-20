<svelte:options runes />
<script lang="ts">
	import { formatDate } from '#lib/forms/record-format.js';
	import CreatorPluginLink from '#lib/components/plugins/CreatorPluginLink.svelte';
	import RecordPluginActions from '#lib/components/plugins/RecordPluginActions.svelte';
	import Button from '#lib/components/Button.svelte';
	import { Pencil, X } from '@lucide/svelte';
	import RecordPluginPanels from '#lib/components/plugins/RecordPluginPanels.svelte';
	import { t } from 'svelte-i18n';
	import RecordDetailLayout from '#lib/components/forms/page/RecordDetailLayout.svelte';
	import RecordChildRecords from '#lib/components/forms/page/RecordChildRecords.svelte';
	import RecordDetailActivity from '#lib/components/forms/page/RecordDetailActivity.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { startEditRecord } = fp.records;
	const { closeRecordDetail } = fp.recordDetail;
	const { reloadSelectedRecord } = fp.recordPanel;
</script>

							<aside
								class="rounded-card border border-line bg-raised"
								data-record-detail-drawer={s.selectedRecord?.id ?? ''}
								data-record-detail-url-record={s.detailUrlRecordId}
							>
								{#if s.selectedRecord}
									<div class="border-b border-line p-4">
										<div class="flex items-start justify-between gap-3">
											<div class="min-w-0">
												<h3 class="truncate text-base font-semibold text-fg">
													{s.selectedRecord.title}
												</h3>

												<p class="mt-1 font-mono text-xs text-fg-muted">
													{s.selectedRecord.id}
												</p>

												<p class="mt-2 text-xs text-fg-muted">
													{$t('forms.updatedAt', {
														values: { date: formatDate(s.selectedRecord.updated_at) }
													})}
												</p>
											</div>

											<div class="flex flex-wrap justify-end gap-2">
												{#if s.selectedForm?.key === 'order_line'}
													<CreatorPluginLink
														workspaceId={s.workspaceId}
														projectId={s.projectId}
														formKey={s.selectedForm.key}
														slot="form.record.actions"
													/>
												{/if}
												<RecordPluginActions
													recordId={s.selectedRecord.id}
													surface={s.recordPluginActions}
												/>
												<Button
													size="sm"
													variant="secondary"
													onclick={() => {
														if (s.selectedRecord) startEditRecord(s.selectedRecord);
													}}
												>
													<Pencil class="mr-2 h-4 w-4" aria-hidden="true" />
													{$t('forms.edit')}
												</Button>

												<button
													type="button"
													class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface hover:text-fg"
													aria-label={$t('common.close')}
													onclick={closeRecordDetail}
													data-record-detail-close={s.selectedRecord.id}
													><X class="h-4 w-4" aria-hidden="true" /></button
												>
											</div>
										</div>
									</div>

									{#if s.selectedRecord.project_id === s.projectId}
										{#if s.selectedForm?.key === 'order_line'}
											<CreatorPluginLink
												workspaceId={s.workspaceId}
												projectId={s.projectId}
												formKey={s.selectedForm.key}
												slot="form.record.detail"
											/>
										{/if}
										{#key s.selectedRecord.id}
											<RecordPluginPanels
												recordId={s.selectedRecord.id}
												surface={s.recordPluginActions}
												onApplied={() => void reloadSelectedRecord()}
											/>
										{/key}
									{/if}

									<RecordDetailLayout />

									<div class="border-t border-line p-4">
										<RecordChildRecords />

										<RecordDetailActivity />
									</div>
								{:else}
									<div class="p-6 text-sm text-fg-muted">
										{$t('forms.selectRecordPrompt')}
									</div>
								{/if}
							</aside>
