<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { Activity } from '#lib/api/activity.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { remainingCount } from './paging.js';

	interface Props {
		/** Newest first. */
		activities: Activity[];
		/** Entries the server holds for the work item, loaded or not. */
		total: number;
		loadingMore?: boolean;
		onloadmore: () => void;
		/** The words of one entry, and who did it and when. */
		text: (activity: Activity) => string;
		author: (activity: Activity) => string;
		date: (value: string) => string;
	}

	let { activities, total, loadingMore = false, onloadmore, text, author, date }: Props = $props();

	const remaining = $derived(remainingCount(total, activities.length));
</script>

<div class="rounded-card border border-line bg-raised p-5" data-testid="activity-panel">
	<h3 class="mb-3 text-sm font-semibold text-fg">
		{$t('issue.activity')}
	</h3>
	<div class="space-y-3">
		{#if activities.length === 0}
			<p class="text-xs text-fg-muted">{$t('issue.noActivity')}</p>
		{:else}
			{#each activities as activity, index (activity.id)}
				<div class="flex items-start gap-3 py-2" data-testid="activity-entry">
					<div class="flex min-h-10 flex-col items-center">
						<div class="h-2 w-2 rounded-full bg-fg-muted"></div>
						<div
							class="w-0.5 flex-1 bg-line"
							class:opacity-0={index === activities.length - 1 && remaining === 0}
						></div>
					</div>
					<div>
						<p class="text-sm text-fg">
							<span class="font-medium">{author(activity)}</span>
							<!-- eslint-disable-next-line svelte/no-useless-mustaches -- keeps the space between the author and the activity text -->
							{' '}
							{text(activity)}
						</p>
						<p class="text-xs text-fg-muted">
							{date(activity.created_at)}
						</p>
					</div>
				</div>
			{/each}
			{#if remaining > 0}
				<Button size="sm" loading={loadingMore} onclick={onloadmore}>
					{$t('issue.loadMoreActivity', { values: { count: remaining } })}
				</Button>
			{/if}
		{/if}
	</div>
</div>
