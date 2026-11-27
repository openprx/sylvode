<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { aiTasksApi, type AiTask } from '../../api/ai-tasks';
	import Button from '../../ui/button/Button.svelte';
	import StateBlock from '../../ui/state-block/StateBlock.svelte';
	import TaskRow from './TaskRow.svelte';

	interface Props {
		issueId: string;
		/** Member names by user id, from the members list the issue page already loads. */
		names: Record<string, string>;
		/** Changes whenever the page wants the list reloaded (after a comment is posted). */
		refreshKey?: number;
	}

	let { issueId, names, refreshKey = 0 }: Props = $props();

	/** Tasks shown at first and added by each "show more". */
	const PAGE_SIZE = 5;

	let tasks = $state<AiTask[]>([]);
	let total = $state(0);
	let nextPage = $state(2);
	let loading = $state(true);
	let loadingMore = $state(false);
	let failed = $state(false);
	let requestId = 0;

	const remaining = $derived(Math.max(0, total - tasks.length));

	/** Loads the newest page again and drops what was loaded beyond it. */
	async function load(id: string) {
		const mine = ++requestId;
		loading = true;
		const result = await aiTasksApi.listForIssue(id, 1, PAGE_SIZE);
		if (mine !== requestId) return;
		if (result.code === 0 && result.data) {
			tasks = result.data.items;
			total = result.data.total;
			nextPage = 2;
			failed = false;
		} else {
			failed = true;
		}
		loading = false;
	}

	async function loadMore() {
		const mine = requestId;
		loadingMore = true;
		const result = await aiTasksApi.listForIssue(issueId, nextPage, PAGE_SIZE);
		loadingMore = false;
		if (mine !== requestId) return;
		if (result.code === 0 && result.data) {
			const known = new Set(tasks.map((task) => task.id));
			tasks = [...tasks, ...result.data.items.filter((task) => !known.has(task.id))];
			total = result.data.total;
			nextPage += 1;
		} else {
			failed = true;
		}
	}

	$effect(() => {
		void refreshKey;
		void load(issueId);
	});
</script>

<!-- Shown only when the work item has agent tasks, or when loading them failed. -->
{#if tasks.length > 0 || failed}
	<section class="panel" data-testid="agent-tasks-panel" aria-labelledby="agent-tasks-title">
		<header class="head">
			<h2 id="agent-tasks-title" class="title">{$t('agentTasks.title')}</h2>
			<Button size="sm" loading={loading && tasks.length > 0} onclick={() => load(issueId)}>
				{$t('agentTasks.refresh')}
			</Button>
		</header>

		{#if failed}
			<StateBlock
				kind="loadFailed"
				thing={$t('agentTasks.things')}
				action={{ run: () => load(issueId) }}
			/>
		{:else}
			<div class="list">
				{#each tasks as task (task.id)}
					<TaskRow {task} {names} />
				{/each}
			</div>
			{#if remaining > 0}
				<Button size="sm" loading={loadingMore} onclick={loadMore}>
					{$t('agentTasks.loadMore', { values: { count: remaining } })}
				</Button>
			{/if}
		{/if}
	</section>
{/if}

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-6);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
		font-family: var(--rf-font-sans-stack);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-subheading);
		line-height: var(--rf-leading-subheading);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.list {
		display: flex;
		flex-direction: column;
	}
</style>
