<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { PluginContribution, PluginPlacement } from '#lib/api/plugin-packages.js';
	import { validPlacementPositions } from '#lib/plugins/placements.js';

	let {
		contributions,
		placements,
		disabled,
		onChange
	}: {
		contributions: PluginContribution[];
		placements: PluginPlacement[];
		disabled: boolean;
		onChange: (id: string, change: Partial<Pick<PluginPlacement, 'enabled' | 'position'>>) => void;
	} = $props();
	const hint = $props.id();
</script>

<fieldset class="space-y-2 rounded border border-slate-300 p-3 dark:border-slate-700" {disabled}>
	<legend class="px-1 text-sm font-medium">{$t('plugins.projectLayout')}</legend>
	<p id={hint} class="text-sm text-slate-600 dark:text-slate-300">
		{$t('plugins.layoutOrderHint')}
	</p>
	{#each contributions as contribution (contribution.id)}
		{@const placement = placements.find((item) => item.id === contribution.id)}
		{#if placement}
			<div
				class="flex flex-wrap items-center justify-between gap-2 border-t border-slate-200 pt-2 text-sm dark:border-slate-700"
			>
				<label class="flex min-w-0 items-start gap-2">
					<input
						type="checkbox"
						class="mt-1"
						checked={placement.enabled}
						onchange={(event) =>
							onChange(contribution.id, { enabled: event.currentTarget.checked })}
					/>
					<span>
						{contribution.label}
						<span class="block text-xs text-slate-600 dark:text-slate-300"
							>{$t(`plugins.slot.${contribution.slot}`)}</span
						>
					</span>
				</label>
				<label class="flex flex-wrap items-center gap-2">
					<span>{$t('plugins.layoutOrder')}</span>
					<input
						type="number"
						min="0"
						max="99"
						step="1"
						class="w-20 rounded border border-slate-300 bg-transparent px-2 py-1 dark:border-slate-600"
						value={Number.isFinite(placement.position) ? placement.position : ''}
						aria-label={$t('plugins.layoutOrderFor', { values: { label: contribution.label } })}
						aria-describedby={hint}
						aria-invalid={!validPlacementPositions([placement])}
						oninput={(event) =>
							onChange(contribution.id, { position: event.currentTarget.valueAsNumber })}
					/>
				</label>
			</div>
		{/if}
	{/each}
	{#if !placements.some((placement) => placement.enabled)}
		<p class="text-sm" role="status">{$t('plugins.layoutAllHidden')}</p>
	{/if}
</fieldset>
