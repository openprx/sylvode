<svelte:options runes />

<script lang="ts">
	import { alertPanelApi, type IssueAlert } from './api';
	import AlertPanelView from './AlertPanelView.svelte';
	import { outcomeOf } from './view';

	interface Props {
		issueId: string;
		workspaceId: string;
		projectId: string;
		/** Owner or admin of the workspace: the rule name links to the Alert rules section. */
		isAdmin?: boolean;
		/** Changes whenever the page wants the facts reloaded (the page bumps it with the tasks). */
		refreshKey?: number;
	}

	let { issueId, workspaceId, projectId, isAdmin = false, refreshKey = 0 }: Props = $props();

	let alert = $state<IssueAlert | null>(null);
	let loaded = $state(false);
	let loading = $state(false);
	let failed = $state(false);
	let requestId = 0;

	async function load(id: string) {
		const mine = ++requestId;
		loading = true;
		const result = await alertPanelApi.getIssueAlert(id);
		if (mine !== requestId) return;
		const outcome = outcomeOf(result.code, result.data !== null);
		if (outcome === 'shown') {
			alert = result.data;
			failed = false;
		} else if (outcome === 'hidden') {
			alert = null;
			failed = false;
		} else {
			failed = true;
		}
		loaded = true;
		loading = false;
	}

	let shownFor = '';

	$effect(() => {
		void refreshKey;
		if (shownFor !== issueId) {
			// A different work item must not keep showing the previous one's facts.
			shownFor = issueId;
			alert = null;
			loaded = false;
			failed = false;
		}
		void load(issueId);
	});
</script>

<!-- Nothing is shown until the first answer, and nothing for a work item that is not an alert. -->
{#if loaded}
	<AlertPanelView
		{alert}
		{failed}
		{loading}
		{workspaceId}
		{projectId}
		{isAdmin}
		onrefresh={() => load(issueId)}
	/>
{/if}
