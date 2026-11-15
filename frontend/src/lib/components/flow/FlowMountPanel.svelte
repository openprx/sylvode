<script lang="ts">
	import { shareLevelKey } from '#lib/flow/share-panel.js';
	import { t } from 'svelte-i18n';
	import { flowApi, type FlowMount } from '#lib/api/flow.js';
	import { workspacesApi, type Workspace } from '#lib/api/workspaces.js';
	import { listWorkspaceProjects, type WorkspaceProjectOption } from '#lib/flow/package-export.js';
	let { objectId }: { objectId: string } = $props();
	let enabled = $state(false);
	let loading = $state(true);
	let busy = $state(false);
	let failed = $state(false);
	let mounts = $state<FlowMount[]>([]);
	let workspaces = $state<Workspace[]>([]);
	let projects = $state<WorkspaceProjectOption[]>([]);
	let workspaceId = $state('');
	let projectId = $state('');
	let level = $state<'view' | 'comment' | 'edit'>('view');
	let removing = $state<string | null>(null);
	let announcement = $state('');
	let intent: { key: string; signature: string } | null = null;
	let generation = 0;
	async function load() {
		const current = ++generation;
		loading = true;
		failed = false;
		try {
			const capabilities = await flowApi.getCapabilities();
			if (current !== generation) return;
			if (capabilities.code !== 0) {
				failed = true;
				return;
			}
			enabled = capabilities.data?.external_mounts === true;
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
		intent = null;
		removing = null;
		workspaceId = '';
		projectId = '';
		projects = [];
		void load();
		return () => {
			generation++;
		};
	});

	async function selectWorkspace(value: string) {
		workspaceId = value;
		projectId = '';
		projects = [];
		if (!value) return;
		const items = await listWorkspaceProjects(value);
		if (workspaceId === value) projects = items;
	}
	async function create(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !workspaceId) return;
		busy = true;
		failed = false;
		const signature = JSON.stringify([workspaceId, projectId, level]);
		if (intent?.signature !== signature) intent = { signature, key: crypto.randomUUID() };
		try {
			const result = await flowApi.createMount(objectId, {
				destination_workspace_id: workspaceId,
				...(projectId ? { destination_project_id: projectId } : {}),
				level,
				idempotency_key: intent.key
			});
			if (result.code !== 0 || !result.data) {
				failed = true;
				return;
			}
			intent = null;
			await load();
			announcement = $t('flow.mounts.created');
		} catch {
			failed = true;
		} finally {
			busy = false;
		}
	}
	async function remove(id: string) {
		if (busy) return;
		busy = true;
		failed = false;
		try {
			const result = await flowApi.removeMount(objectId, id);
			if (result.code !== 0 || !result.data?.removed) {
				failed = true;
				return;
			}
			mounts = mounts.filter((mount) => mount.id !== id);
			removing = null;
			announcement = $t('flow.mounts.removed');
		} catch {
			failed = true;
		} finally {
			busy = false;
		}
	}
	const field =
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-900';
</script>

<section
	class="mt-6 space-y-3 border-t border-slate-200 pt-4 dark:border-slate-700"
	aria-labelledby="flow-mounts-title"
	data-testid="flow-mounts"
>
	<h3 id="flow-mounts-title" class="text-sm font-semibold">{$t('flow.mounts.title')}</h3>
	<p class="text-xs text-slate-500">{$t('flow.mounts.ownership')}</p>
	{#if loading}<p role="status">{$t('flow.mounts.loading')}</p>
	{:else if !enabled && !failed}<p class="text-sm text-slate-500">{$t('flow.mounts.disabled')}</p>
	{:else if enabled}
		<ul class="space-y-2">
			{#each mounts as mount (mount.id)}
				<li class="space-y-2 text-sm">
					<span
						>{workspaces.find((item) => item.id === mount.destination_workspace_id)?.name ??
							mount.destination_workspace_id} · {$t(shareLevelKey(mount.level))}</span
					>
					{#if removing === mount.id}
						<p>{$t('flow.mounts.confirm')}</p>
						<button
							type="button"
							class="min-h-11 px-2 text-red-700"
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
							onclick={() => (removing = mount.id)}>{$t('flow.mounts.remove')}</button
						>{/if}
				</li>
			{:else}<li class="text-sm text-slate-500">{$t('flow.mounts.empty')}</li>{/each}
		</ul>
		<form class="space-y-3" onsubmit={create} aria-busy={busy}>
			<label class="block text-sm" for="mount-workspace">{$t('flow.mounts.workspace')}</label>
			<select
				id="mount-workspace"
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
				<label class="block text-sm" for="mount-project">{$t('flow.mounts.project')}</label>
				<select id="mount-project" class={field} bind:value={projectId} disabled={busy}
					><option value="">{$t('flow.mounts.root')}</option
					>{#each projects as project (project.id)}<option value={project.id}
							>{project.label}</option
						>{/each}</select
				>
			{/if}
			<label class="block text-sm" for="mount-level">{$t('flow.mounts.level')}</label>
			<select id="mount-level" class={field} bind:value={level} disabled={busy}
				>{#each ['view', 'comment', 'edit'] as access (access)}<option value={access}
						>{$t(shareLevelKey(access as 'view' | 'comment' | 'edit'))}</option
					>{/each}</select
			>
			<button
				type="submit"
				class="min-h-11 rounded-md border border-slate-300 px-3 text-sm disabled:opacity-50"
				disabled={busy || !workspaceId}>{$t('flow.mounts.create')}</button
			>
		</form>
	{/if}
	{#if failed}<div role="alert" class="text-sm text-red-700">
			<p>{$t('flow.mounts.failed')}</p>
			<button type="button" class="min-h-11 underline" disabled={busy} onclick={load}
				>{$t('flow.mounts.retry')}</button
			>
		</div>{/if}
	<p class="sr-only" aria-live="polite">{announcement}</p>
</section>
