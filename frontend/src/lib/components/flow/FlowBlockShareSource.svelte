<script lang="ts">
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import Modal from '../Modal.svelte';
	import { apiClient } from '#lib/api/client.js';
	import { workspacesApi, type Workspace, type WorkspaceMember } from '#lib/api/workspaces.js';
	import { projectsApi, type Project } from '#lib/api/projects.js';
	import { authStore } from '#lib/stores/auth.js';
	import { FLOW_REPOSITORY_CONTEXT } from '#lib/flow/context-keys.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { retainedSourceShares } from '#lib/flow/shared-blocks/source-session.js';
	import type { SourceBlockShares, SourceShareSnapshot } from '#lib/flow/shared-blocks/source.js';
	let {
		open = $bindable(false),
		sourceId,
		rootCid = null,
		canManage,
		canRevoke = false
	}: {
		open?: boolean;
		sourceId: string;
		rootCid?: string | null;
		canManage: boolean;
		canRevoke?: boolean;
	} = $props();
	let revokeId = $state<string | null>(null);
	let lifecycleNotice = $state('');
	async function lifecycle(shareId: string, action: 'suspend' | 'revoke') {
		lifecycleNotice = '';
		if (await manager?.changeLifecycle(shareId, action)) {
			revokeId = null;
			lifecycleNotice =
				action === 'revoke' ? 'flow.shared.sourceRevoked' : 'flow.shared.sourcePaused';
		}
	}
	$effect(() => {
		if (!open) {
			revokeId = null;
			lifecycleNotice = '';
		}
	});
	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT),
		workbench = getContext<TaskWorkbench>(WORKBENCH_CONTEXT);
	let manager = $state<SourceBlockShares | null>(null),
		draft = $state<SourceShareSnapshot | null>(null);
	let workspaces = $state<Workspace[]>([]),
		projects = $state<Project[]>([]),
		members = $state<WorkspaceMember[]>([]);
	let targetLoading = $state(false),
		targetFailed = $state(false),
		memberFilter = $state(''),
		workspaceFailed = $state(false);
	let projectPage = $state(1),
		projectPages = $state(1),
		targetGeneration = 0;
	const humans = $derived(members.filter((member) => member.entity_type === 'human'));
	const matching = $derived(
		humans.filter((member) =>
			`${member.name ?? ''} ${member.email ?? ''}`
				.toLowerCase()
				.includes(memberFilter.toLowerCase())
		)
	);
	const admin = $derived(
		members.some(
			(member) => member.user_id === $authStore.user?.id && ['owner', 'admin'].includes(member.role)
		)
	);
	const missing = $derived(
		(draft?.form.recipients ?? []).filter((id) => !humans.some((member) => member.user_id === id))
	);
	const frozen = $derived(!!draft?.busy || draft?.status === 'uncertain');
	const targetValid = $derived(
		admin &&
			!targetLoading &&
			!targetFailed &&
			!!draft?.form.workspaceId &&
			(!draft.form.projectId || projects.some((project) => project.id === draft!.form.projectId)) &&
			(!draft.form.projectId || draft.form.audience === 'users') &&
			(draft.form.audience === 'workspace' ||
				(draft.form.recipients.length > 0 &&
					draft.form.recipients.length <= 128 &&
					!missing.length))
	);
	$effect(() => {
		if (!open) return;
		const binding = retainedSourceShares(workbench, repository, sourceId, canManage);
		if (!binding) return;
		binding.setManage(canManage);
		manager = binding.manager;
		if (rootCid) manager.select(rootCid);
		const release = manager.subscribe((value) => (draft = value));
		if (canManage) {
			void manager.list();
			const epoch = apiClient.getAuthSessionEpoch();
			void workspacesApi.list().then((result) => {
				if (open && apiClient.isAuthSessionEpoch(epoch)) {
					workspaces = result.code === 0 && result.data ? result.data.items : [];
					workspaceFailed = result.code !== 0;
				}
			});
		}
		return release;
	});
	const selectedWorkspace = $derived(draft?.form.workspaceId ?? '');
	$effect(() => {
		const id = selectedWorkspace;
		if (open && id) void loadTarget(id);
	});
	async function loadTarget(id: string): Promise<void> {
		const generation = ++targetGeneration,
			epoch = apiClient.getAuthSessionEpoch();
		targetLoading = true;
		targetFailed = false;
		members = [];
		projects = [];
		projectPage = 1;
		projectPages = 1;
		const [people, places] = await Promise.all([
			workspacesApi.getMembers(id),
			projectsApi.list(id, { page: 1, per_page: 100 })
		]);
		if (!open || generation !== targetGeneration || !apiClient.isAuthSessionEpoch(epoch)) return;
		targetLoading = false;
		targetFailed = people.code !== 0 || places.code !== 0;
		members = people.code === 0 && people.data ? people.data.items : [];
		projects = places.code === 0 && places.data ? places.data.items : [];
		projectPages = places.data?.total_pages ?? 1;
	}
	async function moreProjects(): Promise<void> {
		if (!draft || targetLoading) return;
		const id = draft.form.workspaceId,
			generation = targetGeneration,
			epoch = apiClient.getAuthSessionEpoch(),
			next = projectPage + 1;
		targetLoading = true;
		const result = await projectsApi.list(id, { page: next, per_page: 100 });
		if (!open || generation !== targetGeneration || !apiClient.isAuthSessionEpoch(epoch)) return;
		targetLoading = false;
		if (result.code === 0 && result.data) {
			projects = [...projects, ...result.data.items];
			projectPage = next;
		} else targetFailed = true;
	}
	function discard() {
		manager?.discard();
		if (rootCid) manager?.select(rootCid);
	}
	const error = $derived(
		draft?.error === 'source_changed'
			? $t('flow.shared.sourceChanged')
			: draft?.error === 'not_saved'
				? $t('flow.shared.waitSaved')
				: draft?.error === 'unsupported'
					? $t('flow.shared.sourceUnsupported')
					: draft?.error === 'limit'
						? $t('flow.shared.sourceLimit')
						: draft?.error === 'target'
							? $t('flow.shared.chooseTarget')
							: draft?.error === 'unavailable'
								? $t('flow.shared.sourceUnavailable')
								: $t('flow.shared.publishFailed')
	);
</script>

<Modal bind:open taskScoped title={$t('flow.shared.manageSource')} maxWidthClass="max-w-2xl">
	{#if !canManage}<p role="status" class="text-sm text-slate-500">
			{$t('flow.shared.sourceUnavailable')}
		</p>
	{:else if !draft || !manager}<p role="status" class="text-sm text-slate-500">
			{$t('common.loading')}
		</p>
	{:else}<div class="space-y-5" data-testid="flow-block-sharing-source">
			<p class="text-sm text-slate-500">{$t('flow.shared.sourceBoundary')}</p>
			{#if rootCid && draft.form.rootCid && rootCid !== draft.form.rootCid}<p
					role="status"
					class="text-sm text-amber-700"
				>
					{$t('flow.shared.finishSourceDraft')}
				</p>{/if}
			{#if draft.form.rootCid}<form
					class="space-y-4"
					onsubmit={(event) => {
						event.preventDefault();
						void manager?.review();
					}}
				>
					<fieldset disabled={frozen} class="space-y-2">
						<legend class="text-sm font-medium">{$t('flow.shared.selection')}</legend>
						<label class="flex gap-2 text-sm"
							><input
								type="radio"
								name="share-selection"
								checked={draft.form.selection === 'block'}
								onchange={() => manager?.edit({ selection: 'block' })}
							/>{$t('flow.shared.onlyBlock')}</label
						>
						<label class="flex gap-2 text-sm"
							><input
								type="radio"
								name="share-selection"
								checked={draft.form.selection === 'current_subtree'}
								onchange={() => manager?.edit({ selection: 'current_subtree' })}
							/>{$t('flow.shared.currentSubtree')}</label
						>
					</fieldset>
					<label class="block text-sm"
						>{$t('flow.shared.workspace')}<select
							class="mt-1 block w-full rounded border p-2"
							disabled={frozen || !!draft.form.republishId}
							value={draft.form.workspaceId}
							onchange={(event) =>
								manager?.edit({
									workspaceId: event.currentTarget.value,
									projectId: '',
									recipients: []
								})}
						>
							<option value="">{$t('flow.shared.chooseWorkspace')}</option
							>{#if draft.form.workspaceId && !workspaces.some((workspace) => workspace.id === draft!.form.workspaceId)}<option
									value={draft.form.workspaceId}>{$t('flow.shared.targetUnavailable')}</option
								>{/if}
							{#each workspaces as workspace (workspace.id)}<option value={workspace.id}
									>{workspace.name}</option
								>{/each}
						</select></label
					>
					{#if workspaceFailed}<p role="alert" class="text-sm text-red-600">
							{$t('flow.shared.targetsFailed')}
						</p>{/if}
					{#if draft.form.workspaceId}<label class="block text-sm"
							>{$t('flow.shared.projectDisplay')}<select
								class="mt-1 block w-full rounded border p-2"
								disabled={frozen || !!draft.form.republishId || targetLoading}
								value={draft.form.projectId}
								onchange={(event) =>
									manager?.edit({
										projectId: event.currentTarget.value,
										audience: event.currentTarget.value ? 'users' : draft!.form.audience
									})}
							>
								<option value="">{$t('flow.shared.workspaceDisplay')}</option
								>{#if draft.form.projectId && !projects.some((project) => project.id === draft!.form.projectId)}<option
										value={draft.form.projectId}>{$t('flow.shared.targetUnavailable')}</option
									>{/if}
								{#each projects as project (project.id)}<option value={project.id}
										>{project.name}</option
									>{/each}
							</select></label
						>
						{#if projectPage < projectPages}<button
								type="button"
								class="text-sm underline"
								disabled={targetLoading}
								onclick={moreProjects}>{$t('common.loadMore')}</button
							>{/if}
						{#if targetLoading}<p role="status" class="text-sm text-slate-500">
								{$t('common.loading')}
							</p>{:else if targetFailed}<p role="alert" class="text-sm text-red-600">
								{$t('flow.shared.targetsFailed')}
							</p>{:else if !admin}<p role="status" class="text-sm text-amber-700">
								{$t('flow.shared.targetAdminRequired')}
							</p>{/if}
					{/if}
					<fieldset disabled={frozen || !admin} class="space-y-2">
						<legend class="text-sm font-medium">{$t('flow.shared.audience')}</legend>
						<label class="flex gap-2 text-sm"
							><input
								type="radio"
								name="share-audience"
								checked={draft.form.audience === 'users'}
								onchange={() => manager?.edit({ audience: 'users' })}
							/>{$t('flow.shared.selectedMembers')}</label
						>
						<label class="flex gap-2 text-sm"
							><input
								type="radio"
								name="share-audience"
								disabled={!!draft.form.projectId}
								checked={draft.form.audience === 'workspace'}
								onchange={() => manager?.edit({ audience: 'workspace', recipients: [] })}
							/>{$t('flow.shared.allWorkspaceMembers')}</label
						>
					</fieldset>
					{#if draft.form.audience === 'users'}<div class="space-y-2">
							<label class="block text-sm"
								>{$t('flow.shared.findMember')}<input
									class="mt-1 block w-full rounded border p-2"
									type="search"
									bind:value={memberFilter}
								/></label
							>
							<div class="max-h-48 space-y-2 overflow-y-auto rounded border p-2">
								{#each matching as member (member.user_id)}<label
										class="flex items-center gap-2 text-sm"
										><input
											type="checkbox"
											disabled={frozen ||
												!admin ||
												(!draft.form.recipients.includes(member.user_id) &&
													draft.form.recipients.length >= 128)}
											checked={draft.form.recipients.includes(member.user_id)}
											onchange={(event) =>
												manager?.edit({
													recipients: event.currentTarget.checked
														? [...draft!.form.recipients, member.user_id]
														: draft!.form.recipients.filter((id) => id !== member.user_id)
												})}
										/><span
											>{member.name}<span class="ms-2 text-xs text-slate-500">{member.email}</span
											></span
										></label
									>{/each}
								{#if !matching.length}<p class="text-sm text-slate-500">
										{$t('flow.shared.noMembers')}
									</p>{/if}
							</div>
							{#each missing as id (id)}<div class="flex gap-2 text-sm text-amber-700">
									<span>{$t('flow.shared.memberUnavailable')}</span><button
										type="button"
										class="underline"
										disabled={frozen}
										onclick={() =>
											manager?.edit({
												recipients: draft!.form.recipients.filter((value) => value !== id)
											})}>{$t('common.delete')}</button
									>
								</div>{/each}
						</div>{/if}
					<label class="block text-sm"
						>{$t('flow.shared.permission')}<select
							class="mt-1 block rounded border p-2"
							disabled={frozen}
							value={draft.form.permission}
							onchange={(event) =>
								manager?.edit({ permission: event.currentTarget.value as 'view' | 'edit' })}
							><option value="view">{$t('flow.shared.viewOnly')}</option><option value="edit"
								>{$t('flow.shared.canEditLimited')}</option
							></select
						></label
					>
					<button
						type="submit"
						class="rounded border px-3 py-2 text-sm disabled:opacity-40"
						data-testid="flow-share-review"
						disabled={frozen || !targetValid}
						>{draft.status === 'reviewing'
							? $t('flow.shared.synchronizing')
							: $t('flow.shared.reviewPublication')}</button
					>
				</form>{:else}<p class="text-sm text-slate-500">{$t('flow.shared.chooseBodyBlock')}</p>{/if}
			{#if draft.review}<section
					class="space-y-2 rounded border p-3"
					data-testid="flow-share-fixed-review"
				>
					<h3 class="text-sm font-medium">
						{$t('flow.shared.fixedPreview', { values: { count: draft.review.members.length } })}
					</h3>
					<p class="text-xs text-slate-500">{$t('flow.shared.futureChildrenPrivate')}</p>
					<ol class="max-h-64 space-y-2 overflow-y-auto text-sm">
						{#each draft.review.members as member (member.block_cid)}<li
								style={`margin-inline-start:${member.depth * 16}px`}
								data-review-block-id={member.block_cid}
							>
								{#if member.kind === 'image'}<span class="text-xs text-slate-500"
										>{$t('flow.shared.image')}</span
									>{:else if member.kind === 'attachment'}<span class="text-xs text-slate-500"
										>{$t('flow.shared.file')}</span
									>{:else if member.kind === 'childPage'}<span class="text-xs text-slate-500"
										>{$t('flow.shared.childPlaceholder')}</span
									>{:else if member.kind === 'divider'}<hr />{/if}
								<p class="whitespace-pre-wrap">{member.text}</p>
								{#if member.truncated}<span class="text-xs text-slate-500"
										>{$t('flow.shared.previewShortened')}</span
									>{/if}
							</li>{/each}
					</ol>
					<button
						type="button"
						class="rounded bg-blue-600 px-3 py-2 text-sm text-white disabled:opacity-40"
						data-testid="flow-share-publish"
						disabled={draft.busy || !targetValid}
						onclick={() => manager?.publish()}
						>{draft.busy
							? $t('flow.shared.saving')
							: draft.status === 'uncertain'
								? $t('flow.shared.retryEarlierRequest')
								: draft.form.republishId
									? $t('flow.shared.republish')
									: $t('flow.shared.publish')}</button
					>
				</section>{/if}
			{#if draft.error}<p role="alert" class="text-sm text-red-600">{error}</p>{/if}
			{#if draft.status === 'uncertain'}<p role="status" class="text-sm text-amber-700">
					{$t('flow.shared.uncertainPublication')}
				</p>{:else if draft.status === 'saved'}<p role="status" class="text-sm text-emerald-700">
					{$t('flow.shared.published')}
				</p>{/if}
			{#if draft.dirty}<button
					type="button"
					class="text-sm underline"
					disabled={draft.busy}
					onclick={discard}>{$t('flow.shared.discardSourceDraft')}</button
				>{/if}
			<section class="space-y-3 border-t pt-4">
				<h3 class="text-sm font-medium">{$t('flow.shared.existingShares')}</h3>
				<p role="status">{lifecycleNotice ? $t(lifecycleNotice) : ''}</p>
				{#if !draft.managed.length}<p class="text-sm text-slate-500">
						{$t('flow.shared.noExistingShares')}
					</p>{/if}
				{#each draft.managed as share (share.share_id)}<article
						class="space-y-2 rounded border p-3 text-sm"
						data-share-id={share.share_id}
					>
						<p>
							{$t('flow.shared.fixedPreview', { values: { count: share.member_count } })} · {share.state ===
							'active'
								? $t('flow.shared.sourceActive')
								: share.state === 'suspended'
									? $t('flow.shared.sourcePaused')
									: share.state === 'target_removed'
										? $t('flow.shared.sourceTargetRemoved')
										: $t('flow.shared.sourceRevoked')}
						</p>
						<div class="flex flex-wrap gap-3">
							<button
								type="button"
								class="underline"
								disabled={draft.busy || share.state !== 'active'}
								onclick={() => lifecycle(share.share_id, 'suspend')}
								>{$t('flow.shared.pause')}</button
							><button
								type="button"
								class="underline"
								disabled={draft.busy || draft.dirty || share.state === 'revoked'}
								onclick={() => manager?.republish(share)}
								>{$t('flow.shared.reviewRepublish')}</button
							>
							<a
								class="underline"
								href={resolve('/(app)/shared-blocks/[shareId]', { shareId: share.share_id })}
								>{$t('flow.shared.openAudienceView')}</a
							>
						</div>
						{#if canRevoke && share.state !== 'revoked'}
							{#if revokeId === share.share_id}
								<p>{$t('flow.shared.revokeSourceConfirm')}</p>
								<div class="flex flex-wrap gap-3">
									<button
										type="button"
										class="min-h-11 text-danger underline"
										disabled={draft.busy}
										onclick={() => lifecycle(share.share_id, 'revoke')}
										>{$t('flow.shared.revokeSource')}</button
									>
									<button
										type="button"
										class="min-h-11 underline"
										disabled={draft.busy}
										onclick={() => (revokeId = null)}>{$t('common.cancel')}</button
									>
								</div>
							{:else}
								<button
									type="button"
									class="min-h-11 underline"
									disabled={draft.busy}
									onclick={() => (revokeId = share.share_id)}
									>{$t('flow.shared.revokeSource')}</button
								>
							{/if}
						{/if}
					</article>{/each}
			</section>
			<p class="text-xs text-slate-500">{$t('flow.shared.backgroundPublication')}</p>
		</div>{/if}
</Modal>
