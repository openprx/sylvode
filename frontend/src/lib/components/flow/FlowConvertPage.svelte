<script lang="ts">
	import { getContext, onMount } from 'svelte';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import ConvertWizard from './ConvertWizard.svelte';
	import ConvertPageToForm from './ConvertPageToForm.svelte';
	import { workspacesApi, type Workspace } from '#lib/api/workspaces.js';
	import { FLOW_REPOSITORY_CONTEXT } from '#lib/flow/context-keys.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import { FlowCommandService } from '#lib/flow/command-service.js';
	import {
		conversionAvailability,
		type ConversionAvailability
	} from '#lib/flow/convert-availability.js';
	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commands = new FlowCommandService();
	let sourceTitle = $state('');
	let createForm = $state(false);
	let workspaces = $state<Workspace[]>([]),
		destination = $state('');
	let availability = $state<ConversionAvailability>({ mode: 'unavailable' });
	let loading = $state(true),
		failed = $state(false),
		mounted = $state(false);
	let generation = 0,
		loadedObjectId: string | null = null;
	async function load(objectId: string) {
		const current = ++generation;
		loading = true;
		failed = false;
		workspaces = [];
		destination = '';
		createForm = false;
		sourceTitle = '';
		availability = { mode: 'unavailable' };
		try {
			const source = await repository.get(objectId);
			if (current !== generation) return;
			if (source.code !== 0 || !source.data) {
				failed = true;
				return;
			}
			sourceTitle = source.data.title;
			availability = conversionAvailability(source.data);
			if (availability.mode === 'unavailable') return;
			const allowed = availability;
			const result = await workspacesApi.list();
			if (current !== generation) return;
			failed = result.code !== 0;
			const visible = result.code === 0 ? (result.data?.items ?? []) : [];
			workspaces =
				allowed.mode === 'same_workspace'
					? visible.filter((workspace) => workspace.id === allowed.workspaceId)
					: visible;
			if (allowed.mode === 'same_workspace' && workspaces.length === 1)
				destination = allowed.workspaceId;
		} catch {
			if (current === generation) failed = true;
		} finally {
			if (current === generation) loading = false;
		}
	}
	onMount(() => {
		mounted = true;
		return () => {
			mounted = false;
			++generation;
		};
	});
	$effect(() => {
		const id = page.params.objectId;
		if (mounted && id && id !== loadedObjectId) {
			loadedObjectId = id;
			void load(id);
		}
	});
	const validDestination = $derived(workspaces.some((workspace) => workspace.id === destination));
</script>

<div class="flex-1 overflow-y-auto p-4 md:p-6">
	<div class="mx-auto max-w-3xl space-y-4">
		<h1 class="text-xl font-semibold">{$t('flow.createForm.convertTitle')}</h1>
		{#if sourceTitle}<p class="text-sm font-medium">{sourceTitle}</p>{/if}
		<p class="text-sm text-fg-muted">{$t('flow.personal.destinationNote')}</p>
		{#if loading}<p role="status">{$t('common.loading')}</p>
		{:else if failed}<p role="alert">{$t('flow.personal.conversionAvailabilityFailed')}</p>
			<button
				type="button"
				class="min-h-11 underline"
				onclick={() => page.params.objectId && load(page.params.objectId)}
				>{$t('flow.mounts.retry')}</button
			>
		{:else if availability.mode === 'unavailable'}
			<p
				role="status"
				class="text-sm text-fg-muted"
				data-testid="flow-conversion-unavailable"
			>
				{$t('flow.personal.conversionUnavailable')}
			</p>
		{:else}
			{#if availability.mode === 'same_workspace'}<p class="text-sm text-fg-muted">
					{$t('flow.personal.conversionSameWorkspace')}
				</p>{/if}
			<label for="flow-convert-destination" class="block text-sm font-medium"
				>{$t('flow.mounts.workspace')}</label
			>
			<select
				id="flow-convert-destination"
				bind:value={destination}
				class="min-h-11 w-full rounded-control border border-input bg-field px-3"
			>
				<option value="">{$t('flow.mounts.choose')}</option>
				{#each workspaces as workspace (workspace.id)}<option value={workspace.id}
						>{workspace.name}</option
					>{/each}
			</select>
			{#if workspaces.length === 0}<p class="text-sm text-fg-muted">
					{$t('flow.personal.noDestination')}
				</p>{/if}
			{#if validDestination && page.params.objectId}
				<label class="flex min-h-11 items-center gap-2 text-sm">
					<input type="checkbox" bind:checked={createForm} />{$t('flow.createForm.new')}
				</label>
				{#key `${destination}/${page.params.objectId}/${createForm}`}
					{#if createForm}
						<ConvertPageToForm
							workspaceId={destination}
							objectId={page.params.objectId}
							{repository}
							{commands}
						/>
					{:else}
						<ConvertWizard
							workspaceId={destination}
							objectId={page.params.objectId}
							{repository}
							{commands}
						/>
					{/if}
				{/key}
			{/if}
		{/if}
	</div>
</div>
