<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { FlowObjectView } from '#lib/api/flow.js';
	import type { SyncState } from '#lib/flow/types.js';
	import FlowSyncIndicator from './FlowSyncIndicator.svelte';
	import FlowContextPanel from './FlowContextPanel.svelte';
	let {
		object,
		permittedActions = null,
		activeTool = $bindable(null),
		syncState,
		pending = 0
	}: {
		object: FlowObjectView;
		syncState?: SyncState;
		pending?: number;
		permittedActions?: readonly string[] | null;
		activeTool?: 'info' | 'comments' | 'relations' | 'history' | null;
	} = $props();
	let dialog = $state<HTMLDialogElement | null>(null);
	let shareOpen = $state(false);
	let more = $state<HTMLDetailsElement | null>(null);
	function select(tool: 'info' | 'relations' | 'history') {
		activeTool = tool;
		if (more) more.open = false;
	}
	const MORE_TOOLS = [
		{ id: 'info', labelKey: 'flow.panel.tabs.info' },
		{ id: 'history', labelKey: 'flow.panel.tabs.history' },
		{ id: 'relations', labelKey: 'flow.panel.tabs.relations' }
	] as const;
	const button =
		'min-h-10 rounded-md px-3 text-sm text-slate-600 hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-blue-500 dark:text-slate-300 dark:hover:bg-slate-800';
</script>

<div
	class="flex shrink-0 flex-wrap items-center justify-end gap-1 border-b border-slate-100 bg-white px-3 py-1 dark:border-slate-800 dark:bg-slate-950"
	data-testid="flow-page-tools"
>
	{#if syncState}<FlowSyncIndicator state={syncState} {pending} />{/if}
	<button
		type="button"
		class={button}
		aria-pressed={activeTool === 'comments'}
		data-testid="flow-comments-toggle"
		onclick={() => {
			activeTool = activeTool === 'comments' ? null : 'comments';
			if (more) more.open = false;
		}}>{$t('flow.panel.tabs.comments')}</button
	>
	<button
		type="button"
		class={button}
		data-testid="flow-share-open"
		onclick={() => {
			activeTool = null;
			if (more) more.open = false;
			shareOpen = true;
			dialog?.showModal();
		}}>{$t('flow.panel.tabs.share')}</button
	>
	<details class="relative" bind:this={more}>
		<summary
			class={`${button} cursor-pointer list-none inline-flex items-center`}
			data-testid="flow-more">{$t('flow.layout.more')} ···</summary
		>
		<div
			class="absolute right-0 z-40 min-w-40 rounded-lg border border-slate-200 bg-white p-1 shadow-lg dark:border-slate-700 dark:bg-slate-900"
		>
			{#each MORE_TOOLS as tool (tool.id)}<button
					type="button"
					class={`${button} block w-full text-left`}
					data-testid={`flow-open-${tool.id}`}
					onclick={() => select(tool.id)}>{$t(tool.labelKey)}</button
				>{/each}
		</div>
	</details>
</div>
<dialog
	bind:this={dialog}
	onclose={() => (shareOpen = false)}
	class="m-auto max-h-[85vh] w-[min(92vw,520px)] overflow-auto rounded-xl border border-slate-200 p-0 shadow-xl backdrop:bg-black/30 dark:border-slate-700 dark:bg-slate-900"
	aria-label={$t('flow.panel.tabs.share')}
	data-testid="flow-share-dialog"
>
	{#if shareOpen}<FlowContextPanel
			{object}
			{permittedActions}
			selectedTool="share"
			onclose={() => dialog?.close()}
		/>{/if}
</dialog>
