<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import FormWorkflowTabs from '#lib/components/forms/FormWorkflowTabs.svelte';
	import { ChevronLeft } from '@lucide/svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { handleModeChange } = fp.records;
</script>

{#if s.selectedForm}
	<section class="rounded-card border border-line bg-raised" data-form-config-workbench>
		<div
			class="flex flex-col gap-3 border-b border-line p-4 sm:flex-row sm:items-center sm:justify-between"
		>
			<div>
				<h3 class="text-base font-semibold text-fg">
					{$t('forms.formConfig')}
				</h3>

				<p class="mt-1 text-sm text-fg-muted">
					{$t(`forms.modeContext.${s.activeMode}`, {
						values: { name: s.selectedForm.name }
					})}
				</p>
			</div>

			<Button size="sm" variant="secondary" onclick={() => handleModeChange('data')}>
				<ChevronLeft class="mr-2 h-4 w-4" aria-hidden="true" />
				{$t('forms.backToRecords')}
			</Button>
		</div>

		<FormWorkflowTabs activeMode={s.activeMode} onModeChange={handleModeChange} />
	</section>
{/if}
