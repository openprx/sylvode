<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { pageLinksApi, type PageWorkItems } from '#lib/api/page-links.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import { workflowStateLabel } from '#lib/utils/workflow-state.js';
	import Button from '#lib/ui/button/Button.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { workItemsSummary } from './rules.js';

	interface Props {
		objectId: string;
		/** The page's workspace, which the item links use; items of a page without one are not linked. */
		workspaceId?: string | null;
	}

	let { objectId, workspaceId = null }: Props = $props();

	let data = $state<PageWorkItems | null>(null);
	let failed = $state(false);
	let busyId = $state<string | null>(null);
	let requestId = 0;

	const summary = $derived(data ? workItemsSummary(data) : null);

	async function load(id: string) {
		const mine = ++requestId;
		const result = await pageLinksApi.workItemsOf(id);
		if (mine !== requestId) return;
		if (result.code === 0 && result.data) {
			data = result.data;
			failed = false;
		} else {
			failed = true;
		}
	}

	let shownFor = '';
	$effect(() => {
		if (shownFor !== objectId) {
			shownFor = objectId;
			data = null;
			failed = false;
		}
		void load(objectId);
	});

	async function unlink(itemId: string) {
		busyId = itemId;
		const result = await pageLinksApi.unlink(itemId, objectId);
		busyId = null;
		if (result.code !== 0) {
			toast.error(describeResult(result));
			return;
		}
		toast.success($t('pageLinks.workItems.unlinked'));
		await load(objectId);
	}
</script>

<section
	class="mt-6 space-y-2"
	data-testid="flow-work-items"
	aria-labelledby="flow-work-items-title"
>
	<h2 id="flow-work-items-title" class="text-sm font-semibold text-fg">
		{$t('pageLinks.workItems.title')}
	</h2>
	{#if failed}
		<StateBlock
			kind="loadFailed"
			thing={$t('pageLinks.workItems.thing')}
			action={{ run: () => load(objectId) }}
		/>
	{:else if !data || !summary}
		<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
	{:else}
		{#if summary.empty}
			<p class="text-sm text-fg-muted" data-testid="flow-work-items-empty">
				{$t('pageLinks.workItems.empty')}
			</p>
		{/if}
		{#if data.items.length > 0}
			<ul class="space-y-2" data-testid="flow-work-items-list">
				{#each data.items as item (item.id)}
					<li
						class="flex items-center justify-between gap-2 rounded-control border border-line px-2 py-1 text-sm"
					>
						<span class="min-w-0">
							{#if workspaceId}
								<a
									class="font-medium text-fg underline"
									href={resolve(
										'/(app)/workspace/[workspaceId]/projects/[projectId]/issues/[issueId]',
										{
											workspaceId,
											projectId: item.project_id,
											issueId: item.id
										}
									)}>{item.identifier}</a
								>
							{:else}
								<span class="font-medium text-fg">{item.identifier}</span>
							{/if}
							<span class="ml-1 break-words text-fg">{item.title}</span>
							<span
								class="ml-1 rounded-control border border-line px-1.5 text-xs text-fg-muted"
								data-testid="flow-work-item-state"
								>{workflowStateLabel({ key: item.state }, (key) => $t(key))}</span
							>
						</span>
						<Button
							size="sm"
							loading={busyId === item.id}
							aria-label={$t('pageLinks.workItems.unlinkLabel', {
								values: { identifier: item.identifier }
							})}
							onclick={() => unlink(item.id)}
						>
							{$t('pageLinks.workItems.unlink')}
						</Button>
					</li>
				{/each}
			</ul>
		{/if}
		{#if summary.hiddenCount > 0}
			<p class="text-xs text-fg-muted" data-testid="flow-work-items-hidden">
				{$t('pageLinks.workItems.hidden', { values: { count: summary.hiddenCount } })}
			</p>
		{/if}
		{#if summary.truncated}
			<p class="text-xs text-fg-muted" data-testid="flow-work-items-truncated">
				{$t('pageLinks.workItems.truncated')}
			</p>
		{/if}
	{/if}
</section>
