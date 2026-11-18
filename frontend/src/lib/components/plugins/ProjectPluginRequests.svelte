<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import { pluginLibraryApi, type ProjectPluginRequest } from '#lib/api/plugin-packages.js';
	import Button from '#lib/components/Button.svelte';

	let { projectId, onApproved }: { projectId: string; onApproved: () => Promise<void> } = $props();
	let requests = $state<ProjectPluginRequest[]>([]);
	let offered = $state(false);
	let failed = $state(false);
	let selected = $state<ProjectPluginRequest | null>(null);
	let busy = $state(false);
	let error = $state('');
	let notice = $state('');
	let current = true;

	onMount(() => {
		const signal = apiClient.authSessionSignal();
		const abort = () => {
			current = false;
			requests = [];
			selected = null;
			offered = false;
			failed = false;
		};
		signal.addEventListener('abort', abort, { once: true });
		void load();
		return () => {
			abort();
			signal.removeEventListener('abort', abort);
		};
	});

	async function load() {
		if (!current) return;
		const result = await pluginLibraryApi.requests(projectId);
		if (!current) return;
		offered = result.code === 0 && !!result.data;
		failed = !offered && result.code !== 404 && result.code !== 403;
		if (offered && result.data) requests = result.data.items;
	}

	async function review(approve: boolean) {
		if (!selected || busy || !current) return;
		busy = true;
		error = notice = '';
		const result = await pluginLibraryApi.review(projectId, selected.source_id, approve);
		if (!current) return;
		if (result.code === 0) {
			selected = null;
			notice = approve ? 'plugins.library.approvedNotice' : 'plugins.library.rejectedNotice';
			await Promise.all([load(), approve ? onApproved() : Promise.resolve()]);
		} else {
			error = 'plugins.library.reviewFailed';
			if (result.code === 409 || result.code === 404) {
				selected = null;
				await load();
			}
		}
		busy = false;
	}
</script>

{#if offered || failed}
	<section
		aria-labelledby="plugin-library-requests"
		class="space-y-3 rounded-lg border border-slate-300 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
	>
		<h2 id="plugin-library-requests" class="font-semibold">{$t('plugins.library.requests')}</h2>
		{#if failed}
			<p role="alert" class="text-sm text-red-800 dark:text-red-200">
				{$t('plugins.library.requestsFailed')}
			</p>
			<Button variant="secondary" onclick={load}>{$t('plugins.library.retry')}</Button>
		{:else if !requests.length}
			<p class="text-sm text-slate-600 dark:text-slate-300">{$t('plugins.library.noRequests')}</p>
		{/if}
		{#if requests.length}
			<ul class="divide-y divide-slate-200 dark:divide-slate-700">
				{#each requests as item (item.source_id)}
					<li class="space-y-2 py-3">
						<div class="flex flex-wrap items-center justify-between gap-2">
							<div>
								<h3 class="font-medium">{item.manifest.name} · v{item.manifest.version}</h3>
								<p class="text-sm">{item.manifest.key} · {item.manifest.publisher}</p>
							</div>
							<Button
								variant="secondary"
								size="sm"
								disabled={busy}
								onclick={() => {
									selected = item;
									error = notice = '';
								}}
							>
								{$t('plugins.library.reviewRequest')}
							</Button>
						</div>
						{#if selected?.source_id === item.source_id}
							<div class="space-y-3 border-l-2 border-blue-500 pl-3 text-sm">
								<p>{$t('plugins.library.submittedBy', { values: { owner: item.owner_id } })}</p>
								<p class="break-all font-mono text-xs">SHA-256 {item.package_sha256}</p>
								<p>
									{$t('plugins.creator.targetForm', {
										values: { form: item.manifest.ui.form_key }
									})}
								</p>
								<h4 class="font-medium">{$t('plugins.requestedCapabilities')}</h4>
								<ul class="list-disc pl-5">
									{#each item.manifest.capabilities as capability (capability.name)}
										<li>{$t(`plugins.capability.${capability.name}`)}</li>
									{/each}
								</ul>
								<p>{$t('plugins.library.approvalHelp')}</p>
								<div class="flex flex-wrap gap-2">
									<Button disabled={busy} onclick={() => review(true)}
										>{$t('plugins.library.approve')}</Button
									>
									<Button variant="danger" disabled={busy} onclick={() => review(false)}
										>{$t('plugins.library.reject')}</Button
									>
									<Button
										variant="secondary"
										disabled={busy}
										onclick={() => {
											selected = null;
										}}>{$t('common.cancel')}</Button
									>
								</div>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if error}<p role="alert" class="text-sm text-red-800 dark:text-red-200">{$t(error)}</p>{/if}
		{#if notice}<p role="status" class="text-sm">{$t(notice)}</p>{/if}
	</section>
{/if}
