<script lang="ts">
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import { flowApi } from '#lib/api/flow.js';

	let title = $state('');
	let plainText = $state('');
	let status = $state<'loading' | 'ready' | 'missing'>('loading');

	$effect(() => {
		const token = page.params.token ?? '';
		status = 'loading';
		if (!token) {
			status = 'missing';
			return;
		}
		void flowApi.getPublicShare(token).then((result) => {
			if (page.params.token !== token) return;
			if (result.code === 0 && result.data) {
				title = result.data.title;
				plainText = result.data.plain_text;
				status = 'ready';
			} else {
				status = 'missing';
			}
		});
	});
</script>

<main class="mx-auto max-w-2xl px-4 py-10">
	{#if status === 'loading'}
		<p class="text-slate-500">{$t('flow.publicShare.loading')}</p>
	{:else if status === 'missing'}
		<p class="text-slate-700 dark:text-slate-200">{$t('flow.publicShare.missing')}</p>
	{:else}
		<h1 class="mb-4 text-2xl font-semibold text-slate-900 dark:text-slate-100">
			{title || $t('flow.publicShare.untitled')}
		</h1>
		<p class="whitespace-pre-wrap text-slate-800 dark:text-slate-100">{plainText}</p>
	{/if}
</main>
