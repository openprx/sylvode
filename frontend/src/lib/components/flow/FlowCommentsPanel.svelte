<script lang="ts">
	import { t } from 'svelte-i18n';
	import { getContext, untrack, onDestroy } from 'svelte';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	const workbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	import { apiClient } from '#lib/api/client.js';
	import { authStore } from '#lib/stores/auth.js';
	import { get } from 'svelte/store';
	import {
		commentActions,
		commentCreationAnchor,
		FlowMutationIntents
	} from '#lib/flow/social-actions.js';
	import { flowApi, type FlowComment, type FlowMentionCandidate } from '#lib/api/flow.js';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';

	interface Props {
		objectId: string;
		canComment?: boolean;
		canEdit?: boolean;
		canManage?: boolean;
		blockId?: string | null;
		nonce?: number;
		revision?: string;
	}

	let {
		objectId,
		canComment = true,
		canEdit = false,
		canManage = false,
		blockId = null,
		nonce = 0,
		revision = ''
	}: Props = $props();

	let items = $state<FlowComment[]>([]);
	let truncated = $state(false);
	let loading = $state(true);
	let requestGeneration = 0;
	let destroyed = false;
	onDestroy(() => {
		destroyed = true;
		requestGeneration++;
	});
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

	const intents = new FlowMutationIntents();
	function actions(comment: FlowComment) {
		return commentActions(comment, get(authStore).user?.id ?? null, {
			comment: canComment,
			edit: canEdit,
			manage: canManage
		});
	}
	function operation(kind: string, id = '') {
		return `${get(authStore).user?.id ?? ''}:${objectId}:${kind}:${id}`;
	}

	const roots = $derived(items.filter((item) => !item.parent_id));
	const visibleRoots = $derived(
		anchor
			? roots.filter((item) => item.anchor_status !== 'orphaned' && item.block_id === anchor)
			: roots
	);
	const anchoredBlocks = $derived([
		...new Set(
			roots
				.filter((item) => item.anchor_status !== 'orphaned')
				.map((item) => item.block_id)
				.filter((id): id is string => !!id)
		)
	]);

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
		return loadFor(objectId);
	}

	$effect(() => {
		const id = objectId;
		items = [];
		const saved = untrack(() =>
			workbench?.snapshot<{
				body: string;
				replyTo: string | null;
				editingId: string | null;
				editBody: string;
				anchor: string | null;
				picked: FlowMentionCandidate[];
			}>(`flow:${id}::comment-draft`)
		);
		body = saved?.body ?? '';
		replyTo = saved?.replyTo ?? null;
		editingId = saved?.editingId ?? null;
		editBody = saved?.editBody ?? '';
		picked = saved?.picked ?? [];
		anchor = saved?.anchor ?? null;
	});

	$effect(() => {
		if (nonce > 0) anchor = blockId ?? null;
	});

	$effect(() => {
		const draft = { body, replyTo, editingId, editBody, anchor, picked };
		const key = `flow:${objectId}::comment-draft`;
		untrack(() => workbench?.saveSnapshot(key, draft));
	});

	$effect(() => {
		const id = objectId;
		void revision;
		untrack(() => {
			items = [];
			void loadFor(id);
		});
	});
	async function loadFor(id: string): Promise<void> {
		const version = ++requestGeneration,
			context = revision,
			epoch = apiClient.getAuthSessionEpoch();
		loading = true;
		const result = await flowApi.listComments(id);
		if (
			destroyed ||
			version !== requestGeneration ||
			id !== objectId ||
			context !== revision ||
			!apiClient.isAuthSessionEpoch(epoch)
		)
			return;
		if (result.code === 0 && result.data) {
			items = result.data.items;
			truncated = result.data.truncated;
			failureKey = null;
		} else {
			items = [];
			fail(result);
		}
		loading = false;
	}

	async function submit(): Promise<void> {
		if (busy || !canComment) return;
		busy = true;
		failureKey = null;
		const payload = {
			body,
			parent_id: replyTo,
			block_id: commentCreationAnchor(anchor, replyTo, items),
			mentions: picked.map((person) => person.user_id)
		};
		const op = operation('create');
		const result = await flowApi.createComment(objectId, payload, intents.key(op, payload));
		busy = false;
		if (result.code !== 0 || !result.data) {
			fail(result);
			return;
		}
		intents.complete(op);
		body = '';
		picked = [];
		replyTo = null;
		await load();
	}

	async function saveEdit(commentId: string): Promise<void> {
		const comment = items.find((item) => item.id === commentId);
		if (busy || !comment || !actions(comment).edit) return;
		busy = true;
		const payload = { body: editBody };
		const op = operation('edit', commentId);
		const result = await flowApi.updateComment(
			objectId,
			commentId,
			payload,
			intents.key(op, payload)
		);
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		intents.complete(op);
		editingId = null;
		await load();
	}

	async function remove(commentId: string): Promise<void> {
		const comment = items.find((item) => item.id === commentId);
		if (busy || !comment || !actions(comment).delete) return;
		busy = true;
		const op = operation('delete', commentId);
		const result = await flowApi.deleteComment(objectId, commentId, intents.key(op, {}));
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		intents.complete(op);
		await load();
	}

	async function resolve(commentId: string, resolved: boolean): Promise<void> {
		const comment = items.find((item) => item.id === commentId);
		if (busy || !comment || !actions(comment).resolve) return;
		busy = true;
		const op = operation('resolve', commentId);
		const result = await flowApi.resolveComment(
			objectId,
			commentId,
			resolved,
			intents.key(op, { resolved })
		);
		busy = false;
		if (result.code !== 0) {
			fail(result);
			return;
		}
		intents.complete(op);
		await load();
	}
</script>

{#if !canComment}<p class="mb-3 text-xs text-fg-muted" role="status">
		{$t('flow.personal.commentsReadOnly')}
	</p>{/if}

<div class="space-y-3 text-sm" data-testid="flow-comments">
	{#if loading && items.length === 0}
		<p class="text-fg-muted">{$t('flow.comments.loading')}</p>
	{:else if visibleRoots.length === 0}
		<p class="text-fg-muted">{$t('flow.comments.empty')}</p>
	{:else}
		<ul class="space-y-3">
			{#each visibleRoots as comment (comment.id)}
				<li class="rounded-card border border-line p-2">
					{#if comment.anchor_status === 'orphaned'}<p
							class="mb-1 text-xs text-fg-muted"
							data-comment-anchor-status="orphaned"
						>
							{$t('flow.comments.anchorUnavailable')}
						</p>{/if}
					{#if editingId === comment.id}
						<textarea
							class="min-h-16 w-full rounded-control border border-input bg-field p-2 text-fg"
							bind:value={editBody}
							readonly={!actions(comment).edit}></textarea>
						<button
							type="button"
							class="mr-2 text-fg underline"
							disabled={busy || !actions(comment).edit}
							onclick={() => saveEdit(comment.id)}>{$t('flow.comments.save')}</button
						>
						<button type="button" onclick={() => (editingId = null)}>{$t('common.cancel')}</button>
					{:else}
						<p class="whitespace-pre-wrap text-fg">{comment.body}</p>
						{#if comment.resolved_at}
							<p class="text-xs text-fg-muted">{$t('flow.comments.resolved')}</p>
						{/if}
						{#if canComment}
							<div class="mt-1 flex flex-wrap gap-2 text-xs">
								<button type="button" onclick={() => (replyTo = comment.id)}
									>{$t('flow.comments.reply')}</button
								>
								{#if actions(comment).edit}<button
										type="button"
										onclick={() => {
											editingId = comment.id;
											editBody = comment.body;
										}}>{$t('flow.comments.edit')}</button
									>{/if}
								{#if actions(comment).delete}<button
										type="button"
										onclick={() => remove(comment.id)}>{$t('flow.comments.delete')}</button
									>{/if}
								{#if actions(comment).resolve}<button
										type="button"
										onclick={() => resolve(comment.id, !comment.resolved_at)}
									>
										{comment.resolved_at ? $t('flow.comments.reopen') : $t('flow.comments.resolve')}
									</button>{/if}
							</div>
						{/if}
					{/if}
					{#if repliesOf(comment.id).length > 0}
						<ul class="mt-2 space-y-2 border-l border-line pl-3">
							{#each repliesOf(comment.id) as reply (reply.id)}
								<li>
									<p class="whitespace-pre-wrap text-fg">{reply.body}</p>
									{#if reply.anchor_status === 'orphaned'}<p
											class="text-xs text-fg-muted"
											data-comment-anchor-status="orphaned"
										>
											{$t('flow.comments.anchorUnavailable')}
										</p>{/if}
									{#if actions(reply).delete}<button
											type="button"
											class="text-xs"
											disabled={busy}
											onclick={() => remove(reply.id)}>{$t('flow.comments.delete')}</button
										>{/if}
								</li>
							{/each}
						</ul>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
	{#if truncated}
		<p class="text-xs text-fg-muted">{$t('flow.comments.truncated')}</p>
	{/if}
	{#if replyTo}<p role="status" class="text-xs text-fg-muted">
			{$t('flow.comments.replyingInThread')} · {#if items.find((item) => item.id === replyTo)?.anchor_status === 'orphaned'}{$t(
					'flow.comments.anchorUnavailable'
				)}{/if}<button type="button" class="ms-2 underline" onclick={() => (replyTo = null)}
				>{$t('common.cancel')}</button
			>
		</p>{/if}

	{#if anchor}
		<p class="text-xs text-fg-muted">
			{$t('flow.comments.anchored')}
			<button type="button" class="ml-2 underline" onclick={() => (anchor = null)}
				>{$t('flow.comments.clearAnchor')}</button
			>
		</p>
	{/if}
	{#if anchoredBlocks.length > 0}
		<div class="flex flex-wrap gap-2 text-xs">
			<button type="button" class="underline" onclick={() => (anchor = null)}
				>{$t('flow.comments.page')}</button
			>
			{#each anchoredBlocks as id (id)}
				<button type="button" class="underline" onclick={() => (anchor = id)}>
					{$t('flow.comments.onBlock')}
				</button>
			{/each}
		</div>
	{/if}
	<label class="relative block">
		<span class="mb-1 block text-xs text-fg-muted">{$t('flow.comments.body')}</span>
		<textarea
			class="min-h-16 w-full rounded-control border border-input bg-field p-2 text-fg"
			bind:value={body}
			readonly={!canComment}
			oninput={onBodyInput}></textarea>
		{#if mentionOpen && canComment}
			<ul
				class="absolute z-10 mt-1 max-h-40 w-full overflow-y-auto rounded-float border border-overlay-line bg-overlay shadow-overlay"
				role="listbox"
				aria-label={$t('flow.comments.mention')}
			>
				{#if people.length === 0}
					<li class="px-2 py-1 text-xs text-fg-muted">{$t('flow.comments.noPeople')}</li>
				{/if}
				{#each people as person (person.user_id)}
					<li>
						<button
							type="button"
							class="block w-full px-2 py-1 text-left hover:bg-surface"
							onclick={() => pickPerson(person)}
						>
							{person.name || person.user_id}
							<span class="text-xs text-fg-muted"
								>{person.kind === 'guest'
									? $t('flow.comments.guest')
									: $t('flow.comments.member')}</span
							>
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</label>
	{#if picked.length > 0}
		<ul class="flex flex-wrap gap-1 text-xs">
			{#each picked as person (person.user_id)}
				<li class="rounded bg-surface px-2 py-1">@{person.name}</li>
			{/each}
		</ul>
	{/if}
	<button
		type="button"
		class="min-h-11 rounded-control bg-primary rf-filled px-3 text-on-primary disabled:bg-surface disabled:text-fg-muted"
		disabled={busy || !canComment || body.trim() === ''}
		onclick={submit}
	>
		{$t('flow.comments.submit')}
	</button>
	{#if failureKey}
		<p class="text-danger" role="alert">{$t(failureKey)}</p>
	{/if}
</div>
