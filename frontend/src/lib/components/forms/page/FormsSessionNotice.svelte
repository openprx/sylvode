<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { formsReviewText } from '#lib/forms/record-format.js';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { reloadFormsSession } = fp.session;
</script>

{#if s.formsSessionRefreshFailed}
	<p
		class="rounded border border-line bg-warning-soft p-3 text-sm"
		role="status"
		data-testid="task-refresh-stale"
	>
		{$t('forms.session.refreshFailed')}
	</p>
{/if}
{#if s.formsSessionRemoteChanged || s.formsSessionConflict}
	<section
		class="rounded border border-line bg-warning-soft p-3 text-sm"
		role="status"
		data-testid="task-remote-change"
	>
		<p>{$t('forms.session.remoteChanged')}</p>
		{#if s.formsSessionConflict}<p data-testid="task-conflict">
				{$t('forms.session.conflict')}
			</p>{/if}
		<div class="mt-2 flex gap-2">
			<button
				type="button"
				class="rounded border px-3 py-1"
				data-testid="task-review"
				onclick={() => {
					s.formsSessionReview = !s.formsSessionReview;
				}}>{$t('forms.session.review')}</button
			>
			<button
				type="button"
				class="rounded border px-3 py-1"
				data-testid="task-reload"
				onclick={() => {
					void reloadFormsSession();
				}}>{$t('forms.session.reload')}</button
			>
		</div>
		{#if s.formsSessionReview}
			<div class="mt-3 space-y-2" data-testid="task-review-content">
				{#if s.formsSessionRemoteRecord}
					<table class="w-full text-left">
						<thead
							><tr
								><th>{$t('forms.session.field')}</th><th>{$t('forms.session.mine')}</th><th
									>{$t('forms.session.server')}</th
								></tr
							></thead
						><tbody>
							<tr
								><th>{$t('forms.session.title')}</th><td class="whitespace-pre-wrap"
									>{s.recordTitle}</td
								><td class="whitespace-pre-wrap">{s.formsSessionRemoteRecord.title}</td></tr
							>
							{#each Object.entries(s.recordDraft) as [key, value] (key)}<tr
									><th>{key}</th><td class="whitespace-pre-wrap">{formsReviewText(value)}</td><td
										class="whitespace-pre-wrap"
										>{formsReviewText(s.formsSessionRemoteRecord.values[key])}</td
									></tr
								>{/each}
						</tbody>
					</table>
				{/if}
				<p>{$t('forms.session.reviewReadOnly')}</p>
				<button
					type="button"
					class="rounded border px-3 py-1"
					data-testid="task-keep-draft"
					onclick={() => {
						s.formsSessionReview = false;
					}}>{$t('forms.session.keepDraft')}</button
				>
			</div>
		{/if}
	</section>
{/if}
