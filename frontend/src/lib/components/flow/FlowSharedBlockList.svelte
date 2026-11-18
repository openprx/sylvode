<script lang="ts">
	import { onMount, onDestroy, getContext } from 'svelte';
	import { SvelteMap, SvelteURLSearchParams } from 'svelte/reactivity';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { flowApi } from '#lib/api/flow.js';
	import { apiClient } from '#lib/api/client.js';
	import { workspacesApi } from '#lib/api/workspaces.js';
	import { authStore } from '#lib/stores/auth.js';
	import { sharedBlocksSupported } from '#lib/flow/shared-blocks/capability.js';
	import { sharedBlockApi } from '#lib/flow/shared-blocks/client.js';
	import type {
		SharedDiscovery,
		SharedPlacementManagement
	} from '#lib/flow/shared-blocks/contracts.js';
	import { navigateTask } from '#lib/workbench/navigation.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import FlowSharedBlockCard from './FlowSharedBlockCard.svelte';
	let {
		workspaceId,
		projectId,
		canManageTarget
	}: { workspaceId: string; projectId?: string; canManageTarget?: boolean } = $props();
	const workbench = getContext<TaskWorkbench>(WORKBENCH_CONTEXT);
	const context = $derived({
		workspace_id: workspaceId,
		...(projectId ? { project_id: projectId } : {})
	});
	let enabled = $state(false),
		loading = $state(false),
		failed = $state(false),
		items = $state<SharedDiscovery[]>([]),
		limit = $state(20),
		confirmRemove = $state<string | null>(null),
		removing = $state<string | null>(null),
		ownAdmin = $state(false);
	const admin = $derived(canManageTarget ?? ownAdmin);
	let management = $state<SharedPlacementManagement[]>([]),
		managementOpen = $state(false),
		managementMore = $state(false),
		discoveryMore = $state(false);
	let dead = false,
		loadGeneration = 0;
	const abort = new AbortController(),
		keys = new SvelteMap<string, string>();
	async function load(append = false): Promise<void> {
		const generation = ++loadGeneration,
			epoch = apiClient.getAuthSessionEpoch();
		loading = true;
		failed = false;
		const result = await sharedBlockApi.discover(
			context,
			abort.signal,
			append ? items[items.length - 1]?.share_id : undefined
		);
		if (dead || generation !== loadGeneration || !apiClient.isAuthSessionEpoch(epoch)) return;
		loading = false;
		if (result.code === 0 && result.data) {
			items = append ? [...items, ...result.data] : result.data;
			discoveryMore = result.data.length === 50;
		} else {
			items = [];
			failed = true;
		}
	}
	onMount(() => {
		const epoch = apiClient.getAuthSessionEpoch();
		void (async () => {
			const caps = await flowApi.getCapabilities();
			if (dead || !apiClient.isAuthSessionEpoch(epoch)) return;
			enabled = sharedBlocksSupported(caps);
			if (!enabled) return;
			void load();
			if (canManageTarget === undefined) {
				const members = await workspacesApi.getMembers(workspaceId);
				if (!dead && apiClient.isAuthSessionEpoch(epoch))
					ownAdmin =
						members.code === 0 &&
						members.data?.items.some(
							(member) =>
								member.user_id === $authStore.user?.id && ['owner', 'admin'].includes(member.role)
						) === true;
			}
		})();
	});
	onDestroy(() => {
		dead = true;
		abort.abort();
	});
	async function remove(item: Pick<SharedDiscovery, 'share_id' | 'placement_id'>): Promise<void> {
		if (!admin || removing) return;
		const epoch = apiClient.getAuthSessionEpoch(),
			key = keys.get(item.placement_id) ?? crypto.randomUUID();
		keys.set(item.placement_id, key);
		removing = item.share_id;
		const result = await sharedBlockApi.removePlacement(item.share_id, item.placement_id, key);
		if (dead || !apiClient.isAuthSessionEpoch(epoch)) return;
		removing = null;
		if (result.code === 0) {
			keys.delete(item.placement_id);
			confirmRemove = null;
			workbench?.markChanged(`shared-block:${item.share_id}`);
			await load();
			if (managementOpen) await loadManagement();
		} else failed = true;
	}
	async function loadManagement(append = false): Promise<void> {
		if (!admin) return;
		const epoch = apiClient.getAuthSessionEpoch();
		const result = await sharedBlockApi.placements(
			context,
			abort.signal,
			append ? management[management.length - 1]?.placement_id : undefined
		);
		if (dead || !apiClient.isAuthSessionEpoch(epoch)) return;
		if (result.code === 0 && result.data) {
			management = append ? [...management, ...result.data] : result.data;
			managementMore = result.data.length === 50;
		} else {
			management = [];
			failed = true;
		}
	}
	async function moreShares(): Promise<void> {
		limit += 20;
		if (limit > items.length && discoveryMore) await load(true);
	}

	function href(item: SharedDiscovery) {
		const query = new SvelteURLSearchParams({ workspace_id: item.display_context.workspace_id });
		if (item.display_context.project_id) query.set('project_id', item.display_context.project_id);
		return resolve('/(app)/shared-blocks/[shareId]', { shareId: item.share_id }) + '?' + query;
	}
</script>

{#if enabled}<section
		class="mt-6 rounded-lg border border-slate-200 bg-white p-5 dark:border-slate-700 dark:bg-slate-900"
		aria-label={$t('flow.shared.sharedHere')}
		data-testid="flow-shared-target-list"
	>
		<div class="mb-3 flex items-center justify-between gap-3">
			<h2 class="text-lg font-semibold">{$t('flow.shared.sharedHere')}</h2>
			<button type="button" class="text-sm underline" disabled={loading} onclick={() => load()}
				>{$t('flow.shared.refresh')}</button
			>
		</div>
		<p class="mb-3 text-xs text-slate-500">{$t('flow.shared.targetBoundary')}</p>
		{#if failed}<p role="alert" class="mb-3 text-sm text-red-600">
				{$t('flow.shared.targetListFailed')}
			</p>{/if}
		{#if loading && !items.length}<p role="status" class="text-sm text-slate-500">
				{$t('common.loading')}
			</p>{:else if !items.length}<p class="text-sm text-slate-500">
				{$t('flow.shared.noSharedHere')}
			</p>{/if}
		<ul class="space-y-3">
			{#each items.slice(0, limit) as item (item.share_id)}<li
					class="rounded border border-slate-200 p-3 dark:border-slate-700"
					data-share-id={item.share_id}
				>
					<!-- href resolves the app route and appends only target UUID query parameters. -->
					<!-- eslint-disable svelte/no-navigation-without-resolve -->
					<a
						class="block rounded focus-visible:outline-2 focus-visible:outline-blue-500"
						href={resolve('/(app)/shared-blocks/[shareId]', { shareId: item.share_id }) +
							'?' +
							new SvelteURLSearchParams({
								workspace_id: item.display_context.workspace_id,
								...(item.display_context.project_id
									? { project_id: item.display_context.project_id }
									: {})
							})}
						onclick={(event) => {
							if (
								event.button === 0 &&
								!event.metaKey &&
								!event.ctrlKey &&
								!event.shiftKey &&
								!event.altKey
							) {
								event.preventDefault();
								void navigateTask(href(item));
							}
						}}
						>{#key `${item.share_id}:${item.revision}`}<FlowSharedBlockCard
								{item}
								{context}
							/>{/key}</a
					>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
					{#if admin}<div class="mt-2 text-xs">
							{#if confirmRemove === item.share_id}<p class="mb-2 text-amber-700">
									{$t('flow.shared.removeTargetWarning')}
								</p>
								<button
									type="button"
									class="me-3 underline"
									disabled={!!removing}
									onclick={() => remove(item)}>{$t('common.confirm')}</button
								><button
									type="button"
									class="underline"
									disabled={!!removing}
									onclick={() => (confirmRemove = null)}>{$t('common.cancel')}</button
								>{:else}<button
									type="button"
									class="text-slate-500 underline"
									disabled={!!removing}
									onclick={() => (confirmRemove = item.share_id)}
									>{$t('flow.shared.removeTarget')}</button
								>{/if}
						</div>{/if}
				</li>{/each}
		</ul>
		{#if items.length > limit || discoveryMore}<button
				type="button"
				class="mt-3 text-sm underline"
				onclick={moreShares}>{$t('common.loadMore')}</button
			>{/if}
		{#if admin}<details
				class="mt-4 border-t pt-3"
				bind:open={managementOpen}
				ontoggle={(event) => {
					if (event.currentTarget.open) void loadManagement();
				}}
			>
				<summary class="cursor-pointer text-sm">{$t('flow.shared.manageTargetPlacements')}</summary>
				<p class="mt-2 text-xs text-slate-500">{$t('flow.shared.metadataOnly')}</p>
				<ul class="mt-3 space-y-2">
					{#each management as item (item.placement_id)}<li
							class="rounded border p-3 text-sm"
							data-placement-id={item.placement_id}
							data-can-read={item.can_read ? 'true' : 'false'}
						>
							<p>
								{$t('flow.shared.blocks')} · {item.state === 'active'
									? $t('flow.shared.sourceActive')
									: item.state === 'suspended'
										? $t('flow.shared.sourcePaused')
										: item.state === 'target_removed'
											? $t('flow.shared.sourceTargetRemoved')
											: $t('flow.shared.sourceRevoked')}
							</p>
							{#if item.can_read}<a
									class="me-3 underline"
									href={resolve('/(app)/shared-blocks/[shareId]', { shareId: item.share_id })}
									>{$t('flow.shared.openAudienceView')}</a
								>{:else}<p class="text-xs text-slate-500">
									{$t('flow.shared.noContentGrant')}
								</p>{/if}
							{#if confirmRemove === item.share_id}<p class="mt-2 text-xs text-amber-700">
									{$t('flow.shared.removeTargetWarning')}
								</p>
								<button
									type="button"
									class="me-3 underline"
									disabled={!!removing}
									onclick={() => remove(item)}>{$t('common.confirm')}</button
								><button
									type="button"
									class="underline"
									disabled={!!removing}
									onclick={() => (confirmRemove = null)}>{$t('common.cancel')}</button
								>{:else}<button
									type="button"
									class="mt-2 underline"
									disabled={!!removing}
									onclick={() => (confirmRemove = item.share_id)}
									>{$t('flow.shared.removeTarget')}</button
								>{/if}
						</li>{/each}
				</ul>
				{#if managementMore}<button
						type="button"
						class="mt-2 text-sm underline"
						onclick={() => loadManagement(true)}>{$t('common.loadMore')}</button
					>{/if}
			</details>{/if}
	</section>{/if}
