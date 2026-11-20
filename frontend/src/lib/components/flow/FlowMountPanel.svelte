<script lang="ts">
	import { resolve } from '$app/paths';
	import { shareLevelKey } from '#lib/flow/share-panel.js';
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowMount } from '#lib/api/flow.js';
	import { workspacesApi, type Workspace } from '#lib/api/workspaces.js';
	import { listWorkspaceProjects, type WorkspaceProjectOption } from '#lib/flow/package-export.js';
	let { objectId }: { objectId: string } = $props();
	const uid = $props.id();
	let enabled = $state(false);
	let updatesEnabled = $state(false);
	let changing = $state<FlowMount | null>(null);
	let nextLevel = $state<FlowMount['level']>('view');
	let updateIntent: { signature: string; key: string } | null = null;
	let conflict = $state(false);
	let loading = $state(true);
	let busy = $state(false);
	let failed = $state(false);
	let denied = $state(false);
	let projectLoading = $state(false);
	let projectGeneration = 0;
	let mounts = $state<FlowMount[]>([]);
	let workspaces = $state<Workspace[]>([]);
	let projects = $state<WorkspaceProjectOption[]>([]);
	let workspaceId = $state('');
	let projectId = $state('');
	const placed = $derived(
		mounts.some(
			(item) =>
				item.destination_workspace_id === workspaceId &&
				(item.destination_project_id ?? '') === projectId
		)
	);
	let knownProjectNames = $state<Record<string, string>>({});
	let level = $state<'view' | 'comment' | 'edit'>('view');
	let removing = $state<string | null>(null);
	let announcement = $state('');
	let intent: { key: string; signature: string } | null = null;
	let generation = 0;
	async function load() {
		const current = ++generation;
		loading = true;
		failed = false;
		denied = false;
		try {
			const capabilities = await flowApi.getCapabilities();
			if (current !== generation) return;
			if (capabilities.code !== 0) {
				failed = true;
				return;
			}
			enabled = capabilities.data?.external_mounts === true;
			updatesEnabled = capabilities.data?.mount_permission_updates_supported === true;
			if (!enabled) return;
			const [placements, destinations] = await Promise.all([
				flowApi.listMounts(objectId),
				workspacesApi.list()
			]);
			if (current !== generation) return;
			failed = placements.code !== 0 || destinations.code !== 0;
			mounts = placements.data?.items ?? [];
			workspaces = destinations.data?.items ?? [];
		} catch {
			if (current === generation) failed = true;
		} finally {
			if (current === generation) loading = false;
		}
	}
	$effect(() => {
		void objectId;
		busy = false;
		projectLoading = false;
		knownProjectNames = {};
		changing = null;
		updateIntent = null;
		conflict = false;
		announcement = '';
		denied = false;
		intent = null;
		removing = null;
		workspaceId = '';
		projectId = '';
		projects = [];
		void load();
		return () => {
			generation++;
			projectGeneration++;
		};
	});

	async function selectWorkspace(value: string) {
		const change = ++projectGeneration;
		workspaceId = value;
		projectId = '';
		projects = [];
		projectLoading = !!value;
		denied = false;
		if (!value) return;
		try {
			const items = await listWorkspaceProjects(value);
			if (change !== projectGeneration) return;
			projects = items;
			knownProjectNames = {
				...knownProjectNames,
				...Object.fromEntries(items.map((item) => [item.id, item.label]))
			};
		} finally {
			if (change === projectGeneration) projectLoading = false;
		}
	}

	async function create(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !workspaceId || placed || projectLoading) return;
		const current = generation;
		busy = true;
		failed = false;
		denied = false;
		announcement = '';
		const signature = JSON.stringify([workspaceId, projectId, level]);
		if (intent?.signature !== signature) intent = { signature, key: crypto.randomUUID() };
		try {
			const result = await flowApi.createMount(objectId, {
				destination_workspace_id: workspaceId,
				...(projectId ? { destination_project_id: projectId } : {}),
				level,
				idempotency_key: intent.key
			});
			if (current !== generation) return;
			if (result.code !== 0 || !result.data) {
				denied = result.code === 403;
				failed = !denied;
				return;
			}
			intent = null;
			mounts = [...mounts.filter((item) => item.id !== result.data!.id), result.data];
			announcement = $t('flow.mounts.created');
		} catch {
			if (current === generation) failed = true;
		} finally {
			if (current === generation) busy = false;
		}
	}
	function changeAccess(mount: FlowMount) {
		changing = mount;
		nextLevel = mount.level;
		conflict = false;
		announcement = '';
	}

	async function updateAccess(event: SubmitEvent) {
		event.preventDefault();
		if (!changing || !updatesEnabled || busy || nextLevel === changing.level) return;
		const current = generation;
		const mount = changing;
		const signature = JSON.stringify([mount.id, mount.level, nextLevel]);
		if (updateIntent?.signature !== signature)
			updateIntent = { signature, key: crypto.randomUUID() };
		busy = true;
		denied = false;
		failed = false;
		conflict = false;
		announcement = '';
		try {
			const result = await flowApi.updateMount(objectId, mount.id, {
				expected_level: mount.level,
				level: nextLevel,
				idempotency_key: updateIntent.key
			});
			if (current !== generation) return;
			if (result.code !== 0 || !result.data) {
				const details = result.details;
				conflict =
					typeof details === 'object' &&
					details !== null &&
					'reason' in details &&
					details.reason === 'mount_permission_changed';
				denied = result.code === 403;
				failed = !conflict && !denied;
				if (conflict) {
					changing = null;
					updateIntent = null;
					busy = false;
					await load();
				}
				return;
			}
			const updated = result.data;
			mounts = mounts.map((item) => (item.id === updated.id ? updated : item));
			changing = null;
			updateIntent = null;
			announcement = $t('flow.mounts.accessUpdated');
		} catch {
			if (current === generation) failed = true;
		} finally {
			if (current === generation) busy = false;
		}
	}
	async function remove(id: string) {
		if (busy) return;
		const current = generation;
		busy = true;
		failed = false;
		denied = false;
		announcement = '';
		try {
			const result = await flowApi.removeMount(objectId, id);
			if (current !== generation) return;
			if (result.code !== 0 || !result.data?.removed) {
				denied = result.code === 403;
				failed = !denied;
				return;
			}
			mounts = mounts.filter((mount) => mount.id !== id);
			removing = null;
			announcement = $t('flow.mounts.removed');
		} catch {
			if (current === generation) failed = true;
		} finally {
			if (current === generation) busy = false;
		}
	}
	const field = 'min-h-11 w-full rounded-control border border-input bg-field px-2 text-sm';
</script>

<section
	class="mt-6 space-y-3 border-t border-line pt-4"
	aria-labelledby={`${uid}-title`}
	data-testid="flow-mounts"
>
	<h3 id={`${uid}-title`} class="text-sm font-semibold">{$t('flow.mounts.title')}</h3>
	<p class="text-xs text-fg-muted">{$t('flow.mounts.ownership')}</p>
	{#if loading}<p role="status">{$t('flow.mounts.loading')}</p>
	{:else if !enabled && !failed}<p class="text-sm text-fg-muted">{$t('flow.mounts.disabled')}</p>
	{:else if enabled}
		<ul class="space-y-2">
			{#each mounts as mount (mount.id)}
				<li class="space-y-2 text-sm">
					<span
						>{workspaces.find((item) => item.id === mount.destination_workspace_id)?.name ??
							mount.destination_workspace_id} · {mount.destination_project_id
							? (knownProjectNames[mount.destination_project_id] ??
								$t('flow.mounts.projectPlacement'))
							: $t('flow.mounts.root')} · {$t(shareLevelKey(mount.level))}</span
					>
					{#if mount.status === 'active'}
						<a
							class="inline-flex min-h-11 items-center px-2 underline"
							href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
								workspaceId: mount.destination_workspace_id,
								objectId
							})}>{$t('flow.mounts.openDestination')}</a
						>
					{/if}
					{#if updatesEnabled && mount.status === 'active'}
						{#if changing?.id === mount.id}
							<form class="space-y-2" aria-busy={busy} onsubmit={updateAccess}>
								<label class="block" for={`${uid}-new-access`}>{$t('flow.mounts.newAccess')}</label>
								<select
									id={`${uid}-new-access`}
									bind:value={nextLevel}
									disabled={busy}
									class={field}
								>
									{#each ['view', 'comment', 'edit'] as access (access)}
										<option value={access}>{$t(shareLevelKey(access as FlowMount['level']))}</option
										>
									{/each}
								</select>
								<button
									type="submit"
									class="min-h-11 underline"
									disabled={busy || mount.level === nextLevel}
								>
									{$t('flow.mounts.saveAccess')}
								</button>
								<button
									type="button"
									class="min-h-11 underline"
									disabled={busy}
									onclick={() => (changing = null)}
								>
									{$t('common.cancel')}
								</button>
							</form>
						{:else}
							<button
								type="button"
								class="min-h-11 px-2 underline"
								disabled={busy}
								onclick={() => changeAccess(mount)}>{$t('flow.mounts.changeAccess')}</button
							>
						{/if}
					{/if}
					{#if removing === mount.id}
						<p>{$t('flow.mounts.confirm')}</p>
						<button
							type="button"
							class="min-h-11 px-2 text-danger"
							disabled={busy}
							onclick={() => remove(mount.id)}>{$t('flow.mounts.remove')}</button
						>
						<button
							type="button"
							class="min-h-11 px-2"
							disabled={busy}
							onclick={() => (removing = null)}>{$t('flow.mounts.cancel')}</button
						>
					{:else}<button
							type="button"
							class="min-h-11 px-2 underline"
							disabled={busy}
							onclick={() => {
								changing = null;
								removing = mount.id;
							}}>{$t('flow.mounts.remove')}</button
						>{/if}
				</li>
			{:else}<li class="text-sm text-fg-muted">{$t('flow.mounts.empty')}</li>{/each}
		</ul>
		<form class="space-y-3" onsubmit={create} aria-busy={busy}>
			<label class="block text-sm" for={`${uid}-workspace`}>{$t('flow.mounts.workspace')}</label>
			<select
				id={`${uid}-workspace`}
				class={field}
				value={workspaceId}
				disabled={busy}
				onchange={(event) => selectWorkspace(event.currentTarget.value)}
				required
			>
				<option value="">{$t('flow.mounts.choose')}</option
				>{#each workspaces as workspace (workspace.id)}<option value={workspace.id}
						>{workspace.name}</option
					>{/each}
			</select>
			{#if workspaceId}
				<label class="block text-sm" for={`${uid}-project`}>{$t('flow.mounts.project')}</label>
				<select
					id={`${uid}-project`}
					class={field}
					bind:value={projectId}
					disabled={busy || projectLoading}
					><option value="">{$t('flow.mounts.root')}</option
					>{#each projects as project (project.id)}<option value={project.id}
							>{project.label}</option
						>{/each}</select
				>
			{/if}
			<label class="block text-sm" for={`${uid}-level`}>{$t('flow.mounts.level')}</label>
			<select id={`${uid}-level`} class={field} bind:value={level} disabled={busy}
				>{#each ['view', 'comment', 'edit'] as access (access)}<option value={access}
						>{$t(shareLevelKey(access as 'view' | 'comment' | 'edit'))}</option
					>{/each}</select
			>
			<button
				type="submit"
				class="min-h-11 rounded-control border border-fg bg-raised px-3 text-sm disabled:bg-surface disabled:text-fg-muted disabled:border-line"
				disabled={busy || !workspaceId || placed || projectLoading}
				>{$t('flow.mounts.create')}</button
			>
			{#if placed}<p class="text-sm">{$t('flow.mounts.alreadyPlaced')}</p>{/if}
			{#if projectLoading}<p role="status">{$t('common.loading')}</p>{/if}
		</form>
	{/if}
	{#if failed}<div role="alert" class="text-sm text-danger">
			<p>{$t('flow.mounts.failed')}</p>
			<button type="button" class="min-h-11 underline" disabled={busy} onclick={load}
				>{$t('flow.mounts.retry')}</button
			>
		</div>{/if}
	{#if denied}<p role="alert" class="text-sm text-danger">
			{$t('flow.mounts.permissionRequired')}
		</p>{/if}
	{#if conflict}<p role="alert" class="text-sm text-danger">
			{$t('flow.mounts.accessChanged')}
		</p>{/if}
	<p role="status" class="text-sm">{announcement}</p>
</section>
