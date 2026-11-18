<script lang="ts">
	import { onMount, onDestroy, getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { flowApi } from '#lib/api/flow.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { sharedBlocksSupported } from '#lib/flow/shared-blocks/capability.js';
	import {
		retainedSharedSession,
		type SharedViewSession,
		type SharedSessionSnapshot
	} from '#lib/flow/shared-blocks/session.js';
	import type { SharedTargetContext } from '#lib/flow/shared-blocks/contracts.js';
	import FlowSharedProjection from './FlowSharedProjection.svelte';
	import FlowSharedMemberInput from './FlowSharedMemberInput.svelte';
	let { shareId, context }: { shareId: string; context?: SharedTargetContext } = $props();
	const workbench = getContext<TaskWorkbench>(WORKBENCH_CONTEXT);
	let supported = $state<boolean | null>(null),
		session = $state<SharedViewSession | null>(null);
	let viewState = $state<SharedSessionSnapshot>({
		loading: true,
		authorized: false,
		loadError: null,
		draft: null,
		editing: null,
		editor: null,
		issue: null
	});
	let dead = false,
		release = () => {},
		unregister = () => {};
	const remote = $derived(viewState.authorized ? (viewState.draft?.remote ?? null) : null);
	onMount(() => {
		void (async () => {
			const capability = await flowApi.getCapabilities();
			if (dead) return;
			supported = sharedBlocksSupported(capability);
			if (!supported) return;
			session = retainedSharedSession(workbench, shareId);
			if (!session) {
				supported = false;
				return;
			}
			release = session.subscribe((value) => (viewState = value));
			unregister = workbench.register(`shared-block:${shareId}`, {
				capture: () => ({ editingMemberId: viewState.editing?.member.member_id ?? null }),
				dirty: () => session?.dirty() ?? false,
				refresh: () => session?.refresh(context) ?? Promise.resolve()
			});
			await session.refresh(context, true);
		})();
	});
	onDestroy(() => {
		dead = true;
		release();
		unregister();
		viewState.editor?.unmount();
	});
</script>

<svelte:head><title>{$t('flow.shared.blocks')}</title></svelte:head>
<main class="mx-auto w-full max-w-4xl px-6 py-8" data-testid="flow-shared-view">
	<h1 class="text-xl font-semibold">{$t('flow.shared.blocks')}</h1>
	{#if supported === null}<p role="status" class="mt-4 text-sm text-slate-500">
			{$t('common.loading')}
		</p>
	{:else if !supported}<p role="status" class="mt-4 text-sm text-slate-500">
			{$t('flow.shared.unavailableFeature')}
		</p>
	{:else}
		<div class="mt-2 mb-5 flex flex-wrap items-center justify-between gap-3 text-sm text-slate-500">
			<p>{$t('flow.shared.scopeNotice')}</p>
			<button
				type="button"
				class="rounded border px-3 py-1 disabled:opacity-40"
				disabled={viewState.loading || viewState.draft?.phase === 'saving'}
				onclick={() => session?.refresh(context)}>{$t('flow.shared.refresh')}</button
			>
		</div>
		{#if viewState.loading && !remote}<p role="status" class="text-sm text-slate-500">
				{$t('common.loading')}
			</p>
		{:else if !remote}<div
				role="status"
				class="rounded border border-slate-200 p-4 text-sm dark:border-slate-700"
			>
				<p>
					{viewState.loadError === 'failed'
						? $t('flow.shared.loadFailed')
						: $t('flow.shared.unavailable')}
				</p>
				{#if viewState.draft?.commands.length}<p class="mt-2">
						{$t('flow.shared.personalDraftKept')}
					</p>
					<details class="mt-2">
						<summary>{$t('flow.shared.viewPersonalDraft')}</summary
						>{#each viewState.draft.commands as command, index (index)}{#if command.type === 'replace_text'}<pre
									class="mt-2 whitespace-pre-wrap">{command.text}</pre>{/if}{/each}
					</details>{/if}
			</div>
		{:else}
			{#if remote.display_context}<nav
					aria-label={$t('flow.shared.targetLocation')}
					class="mb-3 flex flex-wrap gap-2 text-sm text-slate-500"
				>
					<a
						href={resolve('/(app)/workspace/[workspaceId]', {
							workspaceId: remote.display_context.workspace_id
						})}
						class="underline">{remote.display_context.workspace_name}</a
					>
					{#if remote.display_context.project_id}<span aria-hidden="true">/</span><a
							href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]', {
								workspaceId: remote.display_context.workspace_id,
								projectId: remote.display_context.project_id
							})}
							class="underline">{remote.display_context.project_name}</a
						>{/if}
				</nav>{/if}
			<p class="mb-4 text-xs text-slate-500" data-testid="flow-shared-permission">
				{remote.effective_permission === 'edit'
					? remote.projection.members.some((member) => 'text' in member.content)
						? $t('flow.shared.canEditLimited')
						: $t('flow.shared.noEditableKinds')
					: $t('flow.shared.viewOnly')}
			</p>
			<FlowSharedProjection
				read={remote}
				authorized={() => session?.authorized() ?? false}
				onEdit={(member) => session?.openMember(member.member_id, $t('flow.shared.textEditor'))}
			/>
			{#if viewState.issue}<div
					role="status"
					class="mt-4 rounded border border-amber-300 p-3 text-sm"
				>
					<p>
						{viewState.issue === 'structure'
							? $t('flow.shared.structureNotSupported')
							: viewState.issue === 'finish_composition'
								? $t('flow.shared.finishComposition')
								: viewState.issue === 'unsaved_member'
									? $t('flow.shared.finishCurrentDraft')
									: viewState.issue === 'member_changed'
										? $t('flow.shared.memberChanged')
										: $t('flow.shared.remoteUpdated')}
					</p>
					<button type="button" class="mt-2 underline" onclick={() => session?.dismissIssue()}
						>{$t('flow.recovery.dismiss')}</button
					>
				</div>{/if}
			{#if viewState.draft?.phase === 'conflict'}<section
					role="alert"
					class="mt-4 rounded border border-amber-400 p-4 text-sm"
					data-testid="flow-shared-conflict"
				>
					<p>{$t('flow.shared.conflict')}</p>
					<div class="mt-3 flex flex-wrap gap-3">
						<button
							type="button"
							class="rounded border px-3 py-1"
							disabled={remote.effective_permission !== 'edit'}
							onclick={() => session?.keepDraft()}>{$t('flow.shared.reviewAgainstLatest')}</button
						><button type="button" class="underline" onclick={() => session?.discard()}
							>{$t('flow.shared.discardDraft')}</button
						>
					</div>
				</section>{:else if viewState.draft?.phase === 'failed'}<p
					role="alert"
					class="mt-3 text-sm text-red-600"
				>
					{$t('flow.shared.saveFailed')}
				</p>{/if}
			{#if viewState.draft?.ack}<div
					role="status"
					class="mt-4 rounded border border-slate-200 p-3 text-sm dark:border-slate-700"
				>
					<p>
						{viewState.draft.ack.newerContent
							? $t('flow.shared.savedThenChanged')
							: $t('flow.shared.saved')}
					</p>
					{#if viewState.draft.ack.newerContent}<details class="mt-2">
							<summary>{$t('flow.shared.viewSentDraft')}</summary
							>{#each viewState.draft.lastSent as command, index (index)}{#if command.type === 'replace_text'}<pre
										class="mt-2 whitespace-pre-wrap">{command.text}</pre>{/if}{/each}
						</details>{/if}
					<button type="button" class="mt-2 underline" onclick={() => session?.dismissReceipt()}
						>{$t('flow.recovery.dismiss')}</button
					>
				</div>{/if}
			{#if viewState.editor && viewState.editing && session}{#key viewState.editor}<FlowSharedMemberInput
						editor={viewState.editor}
						{session}
						snapshot={viewState}
					/>{/key}{/if}
		{/if}
	{/if}
</main>
