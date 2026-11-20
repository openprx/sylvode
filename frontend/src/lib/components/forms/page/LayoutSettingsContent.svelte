<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import ViewSetupBasics from '#lib/components/forms/page/ViewSetupBasics.svelte';
	import ViewCardFieldOrder from '#lib/components/forms/page/ViewCardFieldOrder.svelte';
	import ViewKanbanWipPolicy from '#lib/components/forms/page/ViewKanbanWipPolicy.svelte';
	import ViewColumnsDisclosure from '#lib/components/forms/page/ViewColumnsDisclosure.svelte';
	import ViewFilterEditor from '#lib/components/forms/page/ViewFilterEditor.svelte';
	import ViewTypeOptions from '#lib/components/forms/page/ViewTypeOptions.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { viewDraftCardFields } = fp.viewDraft;
	const { viewDraftKanbanWipOptions } = fp.viewDraftFields;
</script>

<section class="w-full" data-view-tools-expanded-panel>
	<div class="space-y-4">
		<div class="rounded-control border border-line bg-raised" data-view-setup>
			<div class="space-y-2 border-t border-line p-3">
				<ViewSetupBasics />

				<div class="space-y-3 rounded-card border border-line bg-nav p-3" data-view-config-panel>
					<div>
						<h4 class="text-sm font-semibold text-fg">
							{$t('forms.viewRules')}
						</h4>

						<p class="mt-1 text-xs text-fg-muted">
							{$t('forms.viewRulesDescription')}
						</p>
					</div>

					<ViewFilterEditor />

					<ViewTypeOptions />

					{#if s.viewDraft.view_type === 'card' && viewDraftCardFields().length > 0}
						<ViewCardFieldOrder />
					{/if}

					{#if viewDraftKanbanWipOptions().length > 0}
						<ViewKanbanWipPolicy />
					{/if}
				</div>
			</div>
		</div>

		<ViewColumnsDisclosure />
	</div>
</section>
