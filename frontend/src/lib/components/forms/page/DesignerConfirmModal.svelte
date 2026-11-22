<svelte:options runes />

<script lang="ts">
	import { AlertTriangle } from '@lucide/svelte';
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { closeDesignerConfirm, confirmDesignerSchemaSave } = fp.designer;
</script>

<Modal
	bind:open={s.designerConfirmOpen}
	title={$t('forms.schemaChangeConfirmTitle')}
	maxWidthClass="max-w-3xl"
	onclose={closeDesignerConfirm}
>
	<div class="space-y-4">
		<div class="rounded-card border border-line bg-warning-soft p-3">
			<div class="flex items-start gap-2 text-sm text-warning">
				<AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
				<p>{$t('forms.schemaChangeConfirmDescription')}</p>
			</div>
		</div>

		<div class="overflow-hidden rounded-control border border-line">
			<table class="min-w-full divide-y divide-line text-sm">
				<thead class="bg-nav text-left text-xs font-medium uppercase text-fg-muted">
					<tr>
						<th class="px-3 py-2">{$t('forms.changeType')}</th>
						<th class="px-3 py-2">{$t('forms.field')}</th>
						<th class="px-3 py-2">{$t('forms.change')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.valueUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.dependencyUsage')}</th>
						<th class="px-3 py-2 text-right">{$t('forms.blockingDependencies')}</th>
					</tr>
				</thead>

				<tbody class="divide-y divide-line bg-raised">
					{#each s.pendingDesignerChanges as change, index (index)}
						<tr>
							<td class="px-3 py-2 text-fg">
								{#if change.kind === 'delete'}
									{$t('forms.changeDelete')}
								{:else if change.kind === 'rename'}
									{$t('forms.changeRename')}
								{:else}
									{$t('forms.changeTypeChange')}
								{/if}
							</td>

							<td class="px-3 py-2">
								<div class="font-medium text-fg">
									{change.fieldLabel}
								</div>

								<div class="font-mono text-xs text-fg-muted">
									{change.fieldKey}
								</div>
							</td>

							<td class="px-3 py-2 font-mono text-xs text-fg-muted">
								{#if change.to}
									{change.from} -> {change.to}
								{:else}
									{change.from}
								{/if}
							</td>

							<td class="px-3 py-2 text-right font-mono text-fg">{change.valueCount}</td>

							<td class="px-3 py-2 text-right font-mono text-fg">{change.dependencyCount}</td>

							<td
								class="px-3 py-2 text-right font-mono {change.blockingDependencyCount > 0
									? 'text-danger'
									: 'text-fg'}">{change.blockingDependencyCount}</td
							>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={closeDesignerConfirm}>{$t('common.cancel')}</Button>

		<Button variant="danger" loading={s.designerSaving} onclick={confirmDesignerSchemaSave}
			>{$t('forms.confirmSchemaSave')}</Button
		>
	{/snippet}
</Modal>
