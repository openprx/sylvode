<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ArrowDown, ArrowUp } from '@lucide/svelte';
	import { detailPageWidgetOption } from '#lib/forms/detail-layout-drafts.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const {
		setDetailPageWidgetEnabled,
		setDetailPageWidgetTitle,
		setDetailPageWidgetDescription,
		setDetailPageWidgetEmptyText,
		setDetailPageWidgetShowCount,
		setDetailPageWidgetHideEmpty,
		setDetailPageWidgetCollapsible,
		setDetailPageWidgetDefaultCollapsed,
		setDetailPageWidgetDensity,
		setDetailPageWidgetLimit,
		moveDetailPageWidget
	} = fp.detailLayoutSave;
</script>

								<section
									class="rounded-card border border-line bg-raised p-4 {s.detailLayoutMode ===
									'widgets'
										? ''
										: 'hidden'}"
									data-detail-page-widget-builder={s.selectedForm?.id ?? ''}
								>
									<div class="mb-3 flex items-start justify-between gap-3">
										<div>
											<h3 class="text-base font-semibold text-fg">
												{$t('forms.detailPageWidgets')}
											</h3>

											<p class="mt-1 text-sm text-fg-muted">
												{$t('forms.detailPageWidgetsDescription')}
											</p>
										</div>

										{#if s.detailPageWidgetSaving}
											<span class="text-xs font-medium text-fg-muted">{$t('common.saving')}</span>
										{/if}
									</div>

									<div class="space-y-2">
										{#each s.selectedDetailPageWidgetEntries as widget, index}
											{@const option = detailPageWidgetOption(widget.key)}

											<div
												class="grid min-h-16 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 rounded-card border border-line px-3 py-2"
												data-detail-page-widget-row={widget.key}
												data-detail-page-widget-enabled={`${widget.key}:${widget.enabled}`}
											>
												<label class="flex h-10 w-10 items-center justify-center">
													<input
														type="checkbox"
														class="h-4 w-4 rounded border-input accent-primary"
														checked={widget.enabled}
														disabled={s.detailPageWidgetSaving}
														aria-label={$t(option.labelKey)}
														data-detail-page-widget-toggle={widget.key}
														onchange={(event) =>
															void setDetailPageWidgetEnabled(
																widget.key,
																event.currentTarget instanceof HTMLInputElement
																	? event.currentTarget.checked
																	: false
															)}
													/>
												</label>

												<div class="min-w-0">
													<div class="truncate text-sm font-medium text-fg">
														{$t(option.labelKey)}
													</div>

													<p class="mt-0.5 text-xs leading-5 text-fg-muted">
														{$t(option.descriptionKey)}
													</p>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetCustomTitle')}</span
														>

														<input
															value={widget.title}
															disabled={s.detailPageWidgetSaving}
															placeholder={$t(option.labelKey)}
															class="w-full rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-title={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetTitle(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
														/>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetDescription')}</span
														>

														<textarea
															value={widget.description}
															disabled={s.detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetDescriptionPlaceholder')}
															class="w-full resize-y rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-description={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDescription(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}></textarea>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetLimit')}</span
														>

														<input
															type="number"
															min="1"
															max="50"
															value={widget.limit}
															disabled={s.detailPageWidgetSaving}
															placeholder={$t('forms.detailPageWidgetLimitDefault')}
															class="w-28 rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-limit={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetLimit(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.value
																		: ''
																)}
														/>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetDensity')}</span
														>

														<select
															value={widget.density}
															disabled={s.detailPageWidgetSaving}
															class="w-40 rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-density={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDensity(
																	widget.key,
																	event.currentTarget instanceof HTMLSelectElement &&
																		event.currentTarget.value === 'compact'
																		? 'compact'
																		: 'comfortable'
																)}
														>
															<option value="comfortable"
																>{$t('forms.detailPageWidgetDensityComfortable')}</option
															>
															<option value="compact"
																>{$t('forms.detailPageWidgetDensityCompact')}</option
															>
														</select>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.showCount}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-show-count={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetShowCount(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetShowCount')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.hideEmpty}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-hide-empty={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetHideEmpty(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetHideEmpty')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.collapsible}
															disabled={s.detailPageWidgetSaving}
															data-detail-page-widget-collapsible={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetCollapsible(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetCollapsible')}</span>
													</label>

													<label class="mt-2 inline-flex items-center gap-2 text-xs text-fg-muted">
														<input
															type="checkbox"
															class="h-4 w-4 rounded border-input accent-primary"
															checked={widget.defaultCollapsed}
															disabled={s.detailPageWidgetSaving || !widget.collapsible}
															data-detail-page-widget-default-collapsed={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetDefaultCollapsed(
																	widget.key,
																	event.currentTarget instanceof HTMLInputElement
																		? event.currentTarget.checked
																		: false
																)}
														/>

														<span>{$t('forms.detailPageWidgetDefaultCollapsed')}</span>
													</label>

													<label class="mt-2 block space-y-1">
														<span class="text-[11px] font-medium uppercase text-fg-muted"
															>{$t('forms.detailPageWidgetEmptyText')}</span
														>

														<textarea
															value={widget.emptyText}
															disabled={s.detailPageWidgetSaving}
															rows="2"
															placeholder={$t('forms.detailPageWidgetEmptyTextPlaceholder')}
															class="w-full resize-y rounded-control border border-input bg-field px-2 py-1.5 text-xs text-fg"
															data-detail-page-widget-empty-text={widget.key}
															onchange={(event) =>
																void setDetailPageWidgetEmptyText(
																	widget.key,
																	event.currentTarget instanceof HTMLTextAreaElement
																		? event.currentTarget.value
																		: ''
																)}></textarea>
													</label>
												</div>

												<div class="flex items-center gap-1">
													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={index === 0 || s.detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetUp')}
														title={$t('forms.moveWidgetUp')}
														data-detail-page-widget-move-up={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, -1)}
														><ArrowUp class="h-4 w-4" aria-hidden="true" /></button
													>

													<button
														type="button"
														class="inline-flex h-8 w-8 items-center justify-center rounded-control border border-line text-fg-muted hover:bg-surface disabled:bg-surface disabled:text-fg-muted"
														disabled={index === s.selectedDetailPageWidgetEntries.length - 1 ||
															s.detailPageWidgetSaving}
														aria-label={$t('forms.moveWidgetDown')}
														title={$t('forms.moveWidgetDown')}
														data-detail-page-widget-move-down={widget.key}
														onclick={() => void moveDetailPageWidget(widget.key, 1)}
														><ArrowDown class="h-4 w-4" aria-hidden="true" /></button
													>
												</div>
											</div>
										{/each}
									</div>
								</section>
