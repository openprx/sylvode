<script lang="ts">
	import { t } from 'svelte-i18n';
	import { TableProperties } from '@lucide/svelte';
	import { formatDate } from '#lib/forms/record-format.js';
	import { formFieldSummary, formListFields } from '#lib/forms/field-options.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { openForm } = fp.session;
</script>

<section class="rounded-card border border-line bg-raised p-4" data-form-list-home>
	<div class="mb-4">
		<div>
			<h2 class="text-lg font-semibold text-fg">
				{$t('forms.formListTitle')}
			</h2>

			<p class="mt-1 text-sm text-fg-muted">
				{$t('forms.formListDescription')}
			</p>
		</div>
	</div>

	<div class="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
		{#each s.forms as form}
			<button
				type="button"
				class="min-h-40 rounded-card border border-line bg-nav p-4 text-left transition-colors hover:border-input hover:bg-surface"
				onclick={() => openForm(form.id)}
			>
				<div class="flex items-start justify-between gap-3">
					<div class="min-w-0">
						<div class="flex items-center gap-2">
							<TableProperties class="h-4 w-4 shrink-0 text-fg-muted" aria-hidden="true" />

							<h3 class="truncate text-base font-semibold text-fg">
								{form.name}
							</h3>
						</div>

						<p class="mt-1 font-mono text-xs text-fg-muted">
							{form.key}
						</p>
					</div>

					<span
						class="rounded bg-raised px-2 py-0.5 text-xs font-medium text-fg-muted ring-1 ring-line"
						>{$t('forms.fieldCount', {
							values: { count: formListFields(form).length }
						})}</span
					>
				</div>

				<p class="mt-3 line-clamp-2 min-h-10 text-sm leading-5 text-fg-muted">
					{form.description || formFieldSummary(form)}
				</p>

				<div
					class="mt-4 flex items-center justify-between gap-3 border-t border-line pt-3 text-xs"
				>
					<span class="truncate text-fg-muted"
						>{$t('forms.updatedAt', { values: { date: formatDate(form.updated_at) } })}</span
					>

					<span class="font-medium text-fg">{$t('forms.openForm')}</span>
				</div>
			</button>
		{/each}
	</div>
</section>
