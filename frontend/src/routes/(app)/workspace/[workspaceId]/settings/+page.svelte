<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { workspacesApi, type Workspace } from '#lib/api/workspaces.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import Card from '#lib/components/Card.svelte';
	import Input from '#lib/components/Input.svelte';
	import Textarea from '#lib/components/Textarea.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import { resolveWorkspaceAdmin } from '#lib/flow/flow-settings.js';
	import ConnectedAppsWorkspace from '#lib/oauth/ConnectedAppsWorkspace.svelte';

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');

	let loading = $state(true);
	let saving = $state(false);
	let deleting = $state(false);
	let workspace = $state<Workspace | null>(null);
	let form = $state({ name: '', slug: '', description: '' });
	let canManageFlow = $state(false);

	onMount(async () => {
		void resolveWorkspaceAdmin(workspaceId).then((isAdmin) => (canManageFlow = isAdmin));
		await loadWorkspace();
	});

	async function loadWorkspace() {
		loading = true;
		const response = await workspacesApi.get(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			workspace = response.data;
			form = {
				name: response.data.name,
				slug: response.data.slug,
				description: response.data.description ?? ''
			};
		}
		loading = false;
	}

	async function saveWorkspace() {
		if (!form.name.trim()) {
			toast.error(get(t)('workspace.enterName'));
			return;
		}
		saving = true;
		const response = await workspacesApi.update(workspaceId, {
			name: form.name.trim(),
			description: form.description.trim() || undefined
		});
		if (response.code !== 0) {
			toast.error(response.message);
		} else {
			toast.success(get(t)('workspace.settingsSaved'));
			await loadWorkspace();
		}
		saving = false;
	}

	async function handleDeleteWorkspace() {
		if (!workspace) return;
		const nameValues = { values: { name: workspace.name } };
		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('workspace.deleteTitle', nameValues),
				body: get(t)('workspace.deleteBody'),
				confirmLabel: get(t)('workspace.deleteAction', nameValues)
			}))
		) {
			return;
		}

		deleting = true;
		const response = await workspacesApi.delete(workspaceId);
		if (response.code !== 0) {
			toast.error(response.message);
			deleting = false;
			return;
		}

		toast.success(get(t)('workspace.deleteSuccess'));
		goto(resolve('/(app)/workspace'));
	}
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">
			{$t('workspace.settingsTitle')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">
			{$t('workspace.settingsSubtitle')}
		</p>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('common.loading')}
		</div>
	{:else}
		<Card>
			<h2 class="text-lg font-semibold text-fg">
				{$t('workspace.basicInfo')}
			</h2>
			<div class="mt-4 grid grid-cols-1 gap-4">
				<Input label={$t('workspace.name')} bind:value={form.name} required />
				<Input
					label={$t('workspace.slug')}
					bind:value={form.slug}
					readonly
					helperText={$t('workspace.slugReadonlyHint')}
				/>
				<Textarea label={$t('workspace.description')} bind:value={form.description} rows={4} />
			</div>
			<div class="mt-4 flex justify-end">
				<Button variant="primary" loading={saving} onclick={saveWorkspace}
					>{$t('settings.save')}</Button
				>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-fg">
				{$t('workspace.labelManagement')}
			</h2>
			<p class="mt-2 text-sm text-fg-muted">
				{$t('workspace.labelManagementDesc')}
			</p>
			<div class="mt-4">
				<Button
					variant="secondary"
					onclick={() => goto(resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId }))}
				>
					{$t('workspace.goToProjectList')}
				</Button>
			</div>
		</Card>

		<Card>
			<div class="flex items-center justify-between">
				<div>
					<h2 class="text-lg font-semibold text-fg">
						{$t('workflow.settingsTitle')}
					</h2>
					<p class="mt-1 text-sm text-fg-muted">
						{$t('workflow.settingsDesc')}
					</p>
				</div>
				<Button
					variant="secondary"
					onclick={() => goto(resolve('/(app)/workspace/[workspaceId]/workflows', { workspaceId }))}
				>
					{$t('workflow.manageWorkflows')}
				</Button>
			</div>
		</Card>

		{#if canManageFlow}
			<Card>
				<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
					<div>
						<h2 class="text-lg font-semibold text-fg">
							{$t('flow.settings.entry.title')}
						</h2>
						<p class="mt-1 text-sm text-fg-muted">
							{$t('flow.settings.entry.description')}
						</p>
					</div>

					<Button
						variant="secondary"
						onclick={() => goto(resolve(`workspace/${workspaceId}/settings/flow`))}
						>{$t('flow.settings.entry.open')}</Button
					>
				</div>
			</Card>
		{/if}

		<ConnectedAppsWorkspace {workspaceId} workspaceName={workspace?.name ?? ''} />

		<Card>
			<h2 class="text-lg font-semibold text-danger">{$t('workspace.dangerZone')}</h2>
			<p class="mt-2 text-sm text-fg-muted">{$t('workspace.deleteDesc')}</p>
			<div class="mt-4">
				<Button variant="danger-quiet" loading={deleting} onclick={handleDeleteWorkspace}
					>{$t('workspace.deleteWorkspace')}</Button
				>
			</div>
		</Card>
	{/if}
</div>
