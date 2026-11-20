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
		pending = 0,
		onManageSharedBlocks
	}: {
		object: FlowObjectView;
		syncState?: SyncState;
		pending?: number;
		onManageSharedBlocks?: () => void;
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
	const button = 'min-h-10 rounded-control px-3 text-sm text-fg-muted hover:bg-surface';
</script>

<div
	class="flex shrink-0 flex-wrap items-center justify-end gap-1 border-b border-line bg-canvas px-3 py-1"
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
			class="absolute right-0 z-[45] min-w-40 rounded-float border border-overlay-line bg-overlay p-1 shadow-overlay"
		>
			{#if onManageSharedBlocks}<button
					type="button"
					class={`${button} block w-full text-left`}
					data-testid="flow-manage-shared-blocks"
					onclick={() => {
						if (more) more.open = false;
						onManageSharedBlocks?.();
					}}>{$t('flow.shared.manageSource')}</button
				>{/if}

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
	class="m-auto max-h-[85vh] w-[min(92vw,520px)] overflow-auto rounded-float border border-overlay-line p-0 shadow-overlay bg-overlay backdrop:bg-scrim"
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
