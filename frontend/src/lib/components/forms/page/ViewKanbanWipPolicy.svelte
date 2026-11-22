<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { inputValue } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { viewDraftKanbanWipOptions, setViewDraftWipLimit } = fp.viewDraftFields;
</script>

<div class="space-y-3 rounded-card border border-line bg-raised p-3">
	<div>
		<h5 class="text-xs font-semibold uppercase text-fg-muted">
			{$t('forms.kanbanWipPolicy')}
		</h5>

		<p class="mt-1 text-xs text-fg-muted">
			{$t('forms.kanbanWipPolicyDescription')}
		</p>
	</div>

	<div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
		{#each viewDraftKanbanWipOptions() as option, index (index)}
			<label class="space-y-1">
				<span class="text-xs font-medium uppercase text-fg-muted"
					>{$t('forms.kanbanWipLimitFor', {
						values: { option }
					})}</span
				>

				<input
					type="number"
					min="0"
					step="1"
					inputmode="numeric"
					placeholder={$t('forms.kanbanNoWipLimit')}
					value={s.viewDraft.wipLimits[option] ?? ''}
					oninput={(event) => setViewDraftWipLimit(option, inputValue(event))}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				/>
			</label>
		{/each}
	</div>
</div>
