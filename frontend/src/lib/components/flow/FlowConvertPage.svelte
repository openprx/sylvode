<script lang="ts">
	import { getContext, onMount } from 'svelte';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import ConvertWizard from './ConvertWizard.svelte';
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
		availability = { mode: 'unavailable' };
		try {
			const source = await repository.get(objectId);
			if (current !== generation) return;
			if (source.code !== 0 || !source.data) {
				failed = true;
				return;
			}
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
		<p class="text-sm text-slate-500">{$t('flow.personal.destinationNote')}</p>
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
				class="text-sm text-slate-600 dark:text-slate-300"
				data-testid="flow-conversion-unavailable"
			>
				{$t('flow.personal.conversionUnavailable')}
			</p>
		{:else}
			{#if availability.mode === 'same_workspace'}<p class="text-sm text-slate-500">
					{$t('flow.personal.conversionSameWorkspace')}
				</p>{/if}
			<label for="flow-convert-destination" class="block text-sm font-medium"
				>{$t('flow.mounts.workspace')}</label
			>
			<select
				id="flow-convert-destination"
				bind:value={destination}
				class="min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 dark:bg-slate-900"
			>
				<option value="">{$t('flow.mounts.choose')}</option>
				{#each workspaces as workspace (workspace.id)}<option value={workspace.id}
						>{workspace.name}</option
					>{/each}
			</select>
			{#if workspaces.length === 0}<p class="text-sm text-slate-500">
					{$t('flow.personal.noDestination')}
				</p>{/if}
			{#if validDestination && page.params.objectId}{#key `${destination}/${page.params.objectId}`}<ConvertWizard
						workspaceId={destination}
						objectId={page.params.objectId}
						{repository}
						{commands}
					/>{/key}{/if}
		{/if}
	</div>
</div>
