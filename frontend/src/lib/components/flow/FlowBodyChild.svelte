<script lang="ts">
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { flowApi } from '#lib/api/flow.js';
	let {
		childId,
		pendingTitle = null,
		publicView = false
	}: { childId: string; pendingTitle?: string | null; publicView?: boolean } = $props();
	let title = $state<string | null>(null),
		childState = $state<'loading' | 'pending' | 'ready' | 'unavailable'>('loading');
	$effect(() => {
		const request = new AbortController();
		const id = childId,
			pending = pendingTitle;
		title = null;
		let active = true;
		if (publicView) {
			childState = 'unavailable';
			return;
		}
		childState = pending ? 'pending' : 'loading';
		if (pending) {
			return () => {
				active = false;
				request.abort();
			};
		}
		void flowApi.getObject(id, {}, request.signal).then((result) => {
			if (!active) return;
			if (result.code === 0 && result.data?.object_type === 'page') {
				title = result.data.title;
				childState = 'ready';
			} else childState = pending ? 'pending' : 'unavailable';
		});
		return () => {
			active = false;
			request.abort();
		};
	});
</script>

<div
	class="my-2 rounded-md border border-line px-3 py-2"
	data-testid="flow-body-child-page"
	data-child-id={publicView ? undefined : childId}
	data-state={publicView ? 'restricted' : childState}
>
	{#if publicView}<p class="text-sm text-slate-500">▧ {$t('flow.body.privatePageLink')}</p>
	{:else if childState === 'ready'}<a
			href={resolve('/(app)/flow/[objectId]', { objectId: childId })}
			class="text-sm underline"
			draggable="false">▧ {title || $t('flow.nav.untitled')}</a
		>
	{:else if childState === 'pending'}<p role="status" class="text-sm text-slate-500">
			▧ {pendingTitle || $t('flow.nav.untitled')} · {$t('flow.body.creatingPage')}
		</p>
	{:else}<p role="status" class="text-sm text-slate-500">
			▧ {$t(publicView ? 'flow.body.privatePageLink' : 'flow.body.pageUnavailable')}
		</p>{/if}
</div>
