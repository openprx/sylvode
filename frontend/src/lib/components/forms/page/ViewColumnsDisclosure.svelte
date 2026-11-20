<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { fieldHasConditionalHidden, fieldHasConditionalReadOnly } from '#lib/forms/field-conditions.js';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { memberFieldHidden, memberFieldLocked } = fp.permissions;
	const { setViewDraftColumn, viewDraftHasColumn } = fp.viewDraft;
</script>

											<details
												class="rounded-card border border-line bg-raised"
												open
												data-view-columns-disclosure
											>
												<summary
													class="flex cursor-pointer list-none items-center justify-between gap-3 px-3 py-2 marker:hidden"
												>
													<span class="text-sm font-semibold text-fg"
														>{$t('forms.visibleColumns')}</span
													>

													<span class="text-xs text-fg-muted"
														>{$t('forms.selectedColumns', {
															values: { count: s.viewDraft.columns.length }
														})}</span
													>
												</summary>

												<div class="border-t border-line p-3">
													<div class="mb-2 flex items-center justify-between gap-3">
														<span class="text-xs font-medium uppercase text-fg-muted"
															>{$t('forms.visibleColumns')}</span
														>

														<span class="text-xs text-fg-muted"
															>{$t('forms.selectedColumns', {
																values: { count: s.viewDraft.columns.length }
															})}</span
														>
													</div>

													<div class="grid gap-2 sm:grid-cols-2">
														{#each s.fields.filter((field) => field.visibility?.list !== false) as field}
															<div
																class="flex items-center gap-2 rounded-card border border-line bg-nav px-3 py-2 text-sm text-fg"
															>
																<input
																	type="checkbox"
																	class="h-4 w-4 rounded border-input accent-primary"
																	checked={viewDraftHasColumn(field.key)}
																	aria-label={`${$t('forms.visibleColumns')}: ${fieldLabel(field)}`}
																	onchange={(event) =>
																		setViewDraftColumn(
																			field.key,
																			event.currentTarget instanceof HTMLInputElement
																				? event.currentTarget.checked
																				: false
																		)}
																/>

																<span class="min-w-0 flex-1 truncate">{fieldLabel(field)}</span>

																{#if memberFieldHidden(field)}
																	<span
																		class="rounded-control bg-warning-soft px-1.5 py-0.5 text-[10px] font-medium uppercase text-warning"
																		data-view-column-permission-hidden={field.key}
																		>{$t('forms.fieldHiddenBadge')}</span
																	>
																{/if}

																{#if memberFieldLocked(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-permission-locked={field.key}
																		>{$t('forms.fieldLockedBadge')}</span
																	>
																{/if}

																{#if fieldHasConditionalHidden(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-conditional-hidden={field.key}
																		>{$t('forms.fieldConditionHiddenBadge')}</span
																	>
																{/if}

																{#if fieldHasConditionalReadOnly(field)}
																	<span
																		class="rounded-control bg-surface px-1.5 py-0.5 text-[10px] font-medium uppercase text-fg-muted"
																		data-view-column-conditional-readonly={field.key}
																		>{$t('forms.fieldConditionReadOnlyBadge')}</span
																	>
																{/if}

																<span
																	class="ml-auto hidden font-mono text-[11px] text-fg-muted 2xl:inline"
																	>{field.key}</span
																>
															</div>
														{/each}
													</div>
												</div>
											</details>
