<script lang="ts">
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowComment, type FlowMentionCandidate } from '#lib/api/flow.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';

	interface Props {
		objectId: string;
		blockId?: string | null;
		nonce?: number;
	}

	let { objectId, blockId = null, nonce = 0 }: Props = $props();

	let items = $state<FlowComment[]>([]);
	let truncated = $state(false);
	let loading = $state(true);
	let body = $state('');
	let replyTo = $state<string | null>(null);
	let anchor = $state<string | null>(null);
	let people = $state<FlowMentionCandidate[]>([]);
	let mentionOpen = $state(false);
	let picked = $state<FlowMentionCandidate[]>([]);
	let editingId = $state<string | null>(null);
	let editBody = $state('');
	let failureKey = $state<string | null>(null);
	let busy = $state(false);

	const roots = $derived(items.filter((item) => !item.parent_id));
	const visibleRoots = $derived(
		anchor ? roots.filter((item) => item.block_id === anchor) : roots
	);
	const anchoredBlocks = $derived(
		[...new Set(roots.map((item) => item.block_id).filter((id): id is string => !!id))]
	);

	function repliesOf(id: string): FlowComment[] {
		return items.filter((item) => item.parent_id === id);
	}

	function mentionQuery(value: string): string | null {
		const match = value.match(/(?:^|\s)@([^\s@]*)$/);
		return match ? match[1] : null;
	}

	async function refreshPeople(query: string): Promise<void> {
		const result = await flowApi.listMentionCandidates(objectId, query);
		if (result.code === 0 && result.data) people = result.data.items;
	}

	function onBodyInput(): void {
		const query = mentionQuery(body);
		mentionOpen = query !== null;
		if (query !== null) void refreshPeople(query);
	}

	function pickPerson(person: FlowMentionCandidate): void {
		if (!picked.some((item) => item.user_id === person.user_id)) picked = [...picked, person];
		body = body.replace(/(^|\s)@[^\s@]*$/, (_match, lead: string) => `${lead}@${person.name} `);
		mentionOpen = false;
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
		picked = [];
		anchor = null;
		void loadFor(id);
	});

	$effect(() => {
		if (nonce > 0) anchor = blockId ?? null;
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
			block_id: anchor,
			mentions: picked.map((person) => person.user_id)
		});
		busy = false;
		if (result.code !== 0 || !result.data) {
			fail(result);
			return;
		}
		body = '';
		picked = [];
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
	{:else if visibleRoots.length === 0}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.comments.empty')}</p>
	{:else}
		<ul class="space-y-3">
			{#each visibleRoots as comment (comment.id)}
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
	{#if anchor}
		<p class="text-xs text-slate-500">
			{$t('flow.comments.anchored')}
			<button type="button" class="ml-2 underline" onclick={() => (anchor = null)}
				>{$t('flow.comments.clearAnchor')}</button
			>
		</p>
	{/if}
	{#if anchoredBlocks.length > 0}
		<div class="flex flex-wrap gap-2 text-xs">
			<button type="button" class="underline" onclick={() => (anchor = null)}>{$t('flow.comments.page')}</button>
			{#each anchoredBlocks as id (id)}
				<button type="button" class="underline" onclick={() => (anchor = id)}>
					{$t('flow.comments.onBlock')}
				</button>
			{/each}
		</div>
	{/if}
	<label class="relative block">
		<span class="mb-1 block text-xs text-slate-500">{$t('flow.comments.body')}</span>
		<textarea
			class="min-h-16 w-full rounded-md border border-slate-300 bg-white p-2 text-slate-900 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100"
			bind:value={body}
			oninput={onBodyInput}
		></textarea>
		{#if mentionOpen}
			<ul
				class="absolute z-10 mt-1 max-h-40 w-full overflow-y-auto rounded-md border border-slate-200 bg-white shadow dark:border-slate-700 dark:bg-slate-900"
				role="listbox"
				aria-label={$t('flow.comments.mention')}
			>
				{#if people.length === 0}
					<li class="px-2 py-1 text-xs text-slate-500">{$t('flow.comments.noPeople')}</li>
				{/if}
				{#each people as person (person.user_id)}
					<li>
						<button type="button" class="block w-full px-2 py-1 text-left hover:bg-slate-100 dark:hover:bg-slate-800" onclick={() => pickPerson(person)}>
							{person.name || person.user_id}
							<span class="text-xs text-slate-500">{person.kind === 'guest' ? $t('flow.comments.guest') : $t('flow.comments.member')}</span>
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</label>
	{#if picked.length > 0}
		<ul class="flex flex-wrap gap-1 text-xs">
			{#each picked as person (person.user_id)}
				<li class="rounded bg-slate-100 px-2 py-1 dark:bg-slate-800">@{person.name}</li>
			{/each}
		</ul>
	{/if}
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
