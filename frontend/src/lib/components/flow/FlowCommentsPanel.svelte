<script lang="ts">
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowComment } from '#lib/api/flow.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';

	interface Props {
		objectId: string;
	}

	let { objectId }: Props = $props();

	let items = $state<FlowComment[]>([]);
	let truncated = $state(false);
	let loading = $state(true);
	let body = $state('');
	let mentions = $state('');
	let replyTo = $state<string | null>(null);
	let editingId = $state<string | null>(null);
	let editBody = $state('');
	let failureKey = $state<string | null>(null);
	let busy = $state(false);

	const roots = $derived(items.filter((item) => !item.parent_id));

	function repliesOf(id: string): FlowComment[] {
		return items.filter((item) => item.parent_id === id);
	}

	function mentionIds(raw: string): string[] {
		const seen = new Set<string>();
		const ids: string[] = [];
		for (const part of raw.split(/[\s,]+/)) {
			const id = part.trim().toLowerCase();
			if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id)) continue;
			if (seen.has(id)) continue;
			seen.add(id);
			ids.push(id);
		}
		return ids;
	}

	function fail(result: { code: number; error_code?: string | null; details?: unknown }): void {
		failureKey = flowErrorDetailKey(flowErrorFromResult(result));
	}

	async function load(): Promise<void> {
		loading = true;
		const result = await flowApi.listComments(objectId);
		if (result.code === 0 && result.data) {
			items = result.data.items;
			truncated = result.data.truncated;
			failureKey = null;
		} else {
			fail(result);
		}
		loading = false;
	}

	$effect(() => {
		const id = objectId;
		items = [];
		body = '';
		replyTo = null;
		editingId = null;
		void loadFor(id);
	});

	async function loadFor(id: string): Promise<void> {
		loading = true;
		const result = await flowApi.listComments(id);
		if (id !== objectId) return;
		if (result.code === 0 && result.data) {
			items = result.data.items;
			truncated = result.data.truncated;
		} else {
			fail(result);
		}
		loading = false;
	}

	async function submit(): Promise<void> {
		if (busy) return;
		busy = true;
		failureKey = null;
		const result = await flowApi.createComment(objectId, {
			body,
			parent_id: replyTo,
			mentions: mentionIds(mentions)
		});
		busy = false;
		if (result.code !== 0 || !result.data) {
			fail(result);
			return;
		}
		body = '';
		mentions = '';
		replyTo = null;
		await load();
	}

	async function saveEdit(commentId: string): Promise<void> {
		if (busy) return;
		busy = true;
		const result = await flowApi.updateComment(objectId, commentId, { body: editBody });
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		editingId = null;
		await load();
	}

	async function remove(commentId: string): Promise<void> {
		if (busy) return;
		busy = true;
		const result = await flowApi.deleteComment(objectId, commentId);
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		await load();
	}

	async function resolve(commentId: string, resolved: boolean): Promise<void> {
		if (busy) return;
		busy = true;
		const result = await flowApi.resolveComment(objectId, commentId, resolved);
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		await load();
	}
</script>

<div class="space-y-3 text-sm" data-testid="flow-comments">
	{#if loading && items.length === 0}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.comments.loading')}</p>
	{:else if roots.length === 0}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.comments.empty')}</p>
	{:else}
		<ul class="space-y-3">
			{#each roots as comment (comment.id)}
				<li class="rounded-md border border-slate-200 p-2 dark:border-slate-700">
					{#if editingId === comment.id}
						<textarea
							class="min-h-16 w-full rounded-md border border-slate-300 bg-white p-2 text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
							bind:value={editBody}
						></textarea>
						<button type="button" class="mr-2 text-blue-600" onclick={() => saveEdit(comment.id)}
							>{$t('flow.comments.save')}</button
						>
						<button type="button" onclick={() => (editingId = null)}>{$t('common.cancel')}</button>
					{:else}
						<p class="whitespace-pre-wrap text-slate-800 dark:text-slate-100">{comment.body}</p>
						{#if comment.resolved_at}
							<p class="text-xs text-slate-500">{$t('flow.comments.resolved')}</p>
						{/if}
						<div class="mt-1 flex flex-wrap gap-2 text-xs">
							<button type="button" onclick={() => (replyTo = comment.id)}>{$t('flow.comments.reply')}</button>
							<button
								type="button"
								onclick={() => {
									editingId = comment.id;
									editBody = comment.body;
								}}>{$t('flow.comments.edit')}</button
							>
							<button type="button" onclick={() => remove(comment.id)}>{$t('flow.comments.delete')}</button>
							<button type="button" onclick={() => resolve(comment.id, !comment.resolved_at)}>
								{comment.resolved_at ? $t('flow.comments.reopen') : $t('flow.comments.resolve')}
							</button>
						</div>
					{/if}
					{#if repliesOf(comment.id).length > 0}
						<ul class="mt-2 space-y-2 border-l border-slate-200 pl-3 dark:border-slate-700">
							{#each repliesOf(comment.id) as reply (reply.id)}
								<li>
									<p class="whitespace-pre-wrap text-slate-800 dark:text-slate-100">{reply.body}</p>
									<button type="button" class="text-xs" onclick={() => remove(reply.id)}
										>{$t('flow.comments.delete')}</button
									>
								</li>
							{/each}
						</ul>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
	{#if truncated}
		<p class="text-xs text-slate-500">{$t('flow.comments.truncated')}</p>
	{/if}
	<label class="block">
		<span class="mb-1 block text-xs text-slate-500">{$t('flow.comments.body')}</span>
		<textarea
			class="min-h-16 w-full rounded-md border border-slate-300 bg-white p-2 text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
			bind:value={body}
		></textarea>
	</label>
	<label class="block">
		<span class="mb-1 block text-xs text-slate-500">{$t('flow.comments.mentions')}</span>
		<input
			class="min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
			bind:value={mentions}
		/>
	</label>
	<button
		type="button"
		class="min-h-11 rounded-md bg-blue-600 px-3 text-white disabled:opacity-50"
		disabled={busy || body.trim() === ''}
		onclick={submit}
	>
		{$t('flow.comments.submit')}
	</button>
	{#if failureKey}
		<p class="text-red-600 dark:text-red-400" role="alert">{$t(failureKey)}</p>
	{/if}
</div>
