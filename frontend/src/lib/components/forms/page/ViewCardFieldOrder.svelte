<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { ArrowDown, ArrowUp } from '@lucide/svelte';
	import { fieldLabel } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { viewDraftCardFields, moveViewDraftCardField } = fp.viewDraft;
</script>

															<div class="space-y-2 rounded-card border border-line bg-raised p-3">
																<div>
																	<h5 class="text-xs font-semibold uppercase text-fg-muted">
																		{$t('forms.cardFieldOrder')}
																	</h5>
																</div>

																<div class="grid gap-2 md:grid-cols-2 xl:grid-cols-3">
																	{#each viewDraftCardFields() as field, fieldIndex}
																		<div
																			class="flex min-w-0 items-center justify-between gap-2 rounded-control border border-line bg-nav px-2 py-1.5 text-sm"
																			data-card-field-order={`${fieldIndex + 1}:${field.key}`}
																		>
																			<span class="min-w-0 truncate text-fg"
																				>{fieldIndex + 1}. {fieldLabel(field)}</span
																			>

																			<div
																				class="flex shrink-0 items-center rounded-control border border-line bg-raised p-0.5"
																			>
																				<button
																					type="button"
																					class="rounded p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					disabled={fieldIndex === 0}
																					onclick={() => moveViewDraftCardField(fieldIndex, -1)}
																					aria-label={`${$t('forms.moveCardFieldUp')} ${fieldLabel(field)}`}
																					data-card-field-move-up={field.key}
																					><ArrowUp
																						class="h-3.5 w-3.5"
																						aria-hidden="true"
																					/></button
																				>

																				<button
																					type="button"
																					class="rounded p-1 text-fg-muted hover:bg-surface hover:text-fg disabled:bg-surface disabled:text-fg-muted"
																					disabled={fieldIndex >= viewDraftCardFields().length - 1}
																					onclick={() => moveViewDraftCardField(fieldIndex, 1)}
																					aria-label={`${$t('forms.moveCardFieldDown')} ${fieldLabel(field)}`}
																					data-card-field-move-down={field.key}
																					><ArrowDown
																						class="h-3.5 w-3.5"
																						aria-hidden="true"
																					/></button
																				>
																			</div>
																		</div>
																	{/each}
																</div>
															</div>
