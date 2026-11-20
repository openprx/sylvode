<script lang="ts">
	import { t } from 'svelte-i18n';
	import { tick } from 'svelte';
	import { navigateTask } from '#lib/workbench/navigation.js';
	import type { TaskWorkbench, TaskTab } from '#lib/workbench/tabs.js';
	let { workbench }: { workbench: TaskWorkbench } = $props();
	let confirming = $state<string | null>(null);
	let dragging = $state<string | null>(null);
	let dropTarget = $state<{ key: string; side: 'before' | 'after' } | null>(null);
	let announcement = $state('');
	let suppressClickUntil = 0;
	let tabList = $state<HTMLDivElement | null>(null);
	function announceMove(key: string) {
		const index = $workbench.tabs.findIndex((tab) => tab.key === key);
		const tab = $workbench.tabs[index];
		if (tab)
			announcement = $t('workbench.moved', {
				values: { title: title(tab), position: index + 1, total: $workbench.tabs.length }
			});
	}
	function scrollDrag(event: DragEvent) {
		if (!dragging || !tabList) return;
		event.preventDefault();
		const bounds = tabList.getBoundingClientRect();
		if (event.clientX < bounds.left + 28) tabList.scrollLeft -= 18;
		else if (event.clientX > bounds.right - 28) tabList.scrollLeft += 18;
	}

	function dragStart(event: DragEvent, key: string) {
		if (confirming) {
			event.preventDefault();
			return;
		}
		dragging = key;
		dropTarget = null;
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
			event.dataTransfer.setData('application/x-sv-task-tab', key);
		}
	}
	function dragOver(event: DragEvent, key: string) {
		if (!dragging || dragging === key) return;
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
		const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
		dropTarget = { key, side: event.clientX < bounds.left + bounds.width / 2 ? 'before' : 'after' };
	}
	function finishDrag() {
		dragging = null;
		dropTarget = null;
		suppressClickUntil = Date.now() + 250;
	}
	function drop(event: DragEvent, key: string) {
		if (!dragging) return;
		event.preventDefault();
		event.stopPropagation();
		const source = dragging;
		const target = dropTarget?.key === key ? dropTarget : null;
		if (target && workbench.moveTabRelative(source, target.key, target.side)) announceMove(source);
		finishDrag();
	}

	const title = (tab: TaskTab) => (tab.title.startsWith('workbench.') ? $t(tab.title) : tab.title);
	const domId = (key: string) => encodeURIComponent(key);
	async function select(tab: TaskTab) {
		if (dragging || Date.now() < suppressClickUntil) return;
		await navigateTask(tab.url);
	}
	async function close(key: string, discard = false) {
		const result = workbench.close(key, discard);
		if (result.kind === 'confirm') {
			confirming = key;
			return;
		}
		confirming = null;
		if (result.kind === 'closed' && result.wasActive) await navigateTask(result.url);
	}
	function onKey(event: KeyboardEvent, index: number) {
		if (confirming) return;
		const tabs = $workbench.tabs;
		if (
			event.altKey &&
			event.shiftKey &&
			(event.key === 'ArrowLeft' || event.key === 'ArrowRight')
		) {
			event.preventDefault();
			const key = tabs[index]?.key;
			if (!key) return;
			if (workbench.moveTab(key, index + (event.key === 'ArrowRight' ? 1 : -1))) announceMove(key);
			void tick().then(() => document.getElementById(`task-tab-${domId(key)}`)?.focus());
			return;
		}
		let target: number | null = null;
		if (event.key === 'ArrowRight') target = (index + 1) % tabs.length;
		if (event.key === 'ArrowLeft') target = (index + tabs.length - 1) % tabs.length;
		if (event.key === 'Home') target = 0;
		if (event.key === 'End') target = tabs.length - 1;
		if (event.key === 'Delete') {
			event.preventDefault();
			void close(tabs[index].key);
			return;
		}
		if (target !== null) {
			event.preventDefault();
			const tab = tabs[target];
			document.getElementById(`task-tab-${domId(tab.key)}`)?.focus();
			void select(tab);
		}
	}
</script>

<p id="task-reorder-hint" class="sr-only">{$t('workbench.reorderHint')}</p>
<div class="sr-only" role="status" aria-live="polite" data-testid="task-tab-reorder-status">
	{announcement}
</div>
{#if $workbench.tabs.length}
	<div
		class="border-b border-slate-200 bg-white dark:border-slate-800 dark:bg-slate-900"
		data-testid="task-workbench"
	>
		<div
			bind:this={tabList}
			ondragover={scrollDrag}
			ondrop={finishDrag}
			role="tablist"
			tabindex="-1"
			aria-label={$t('workbench.openTasks')}
			class="flex min-h-[26px] overflow-x-auto px-3"
			data-testid="task-tablist"
			aria-describedby="task-reorder-hint"
		>
			{#each $workbench.tabs as tab, index (tab.key)}
				<div
					role="presentation"
					data-testid="task-tab-drop-target"
					data-tab-key={tab.key}
					ondragover={(event) => dragOver(event, tab.key)}
					ondrop={(event) => drop(event, tab.key)}
					class="relative flex shrink-0 items-center border-b-2 {dragging === tab.key
						? 'opacity-50'
						: ''} {$workbench.active === tab.key
						? 'border-blue-600 bg-blue-50 dark:bg-blue-950'
						: 'border-transparent'}"
				>
					{#if dropTarget?.key === tab.key}<span
							aria-hidden="true"
							class="pointer-events-none absolute inset-y-1 z-10 w-0.5 rounded bg-blue-600 {dropTarget.side ===
							'before'
								? 'left-0'
								: 'right-0'}"
							data-testid="task-tab-drop-indicator"
							data-drop-side={dropTarget.side}
						></span>{/if}
					<button
						type="button"
						role="tab"
						id={`task-tab-${domId(tab.key)}`}
						aria-selected={$workbench.active === tab.key}
						aria-controls={`task-panel-${domId(tab.key)}`}
						tabindex={$workbench.active === tab.key ? 0 : -1}
						class="min-h-[26px] max-w-56 cursor-grab select-none truncate px-3 text-sm active:cursor-grabbing"
						data-testid="task-tab"
						draggable="true"
						data-dragging={dragging === tab.key}
						ondragstart={(event) => dragStart(event, tab.key)}
						ondragend={finishDrag}
						title={$t('workbench.reorderHint')}
						aria-keyshortcuts="Alt+Shift+ArrowLeft Alt+Shift+ArrowRight"
						data-tab-key={tab.key}
						onclick={() => select(tab)}
						onkeydown={(event) => onKey(event, index)}
					>
						<svg
							aria-hidden="true"
							class="mr-1 inline-block h-3 w-3 align-[-1px] text-slate-400"
							viewBox="0 0 12 12"
							fill="currentColor"
							><circle cx="4" cy="2" r="1" /><circle cx="8" cy="2" r="1" /><circle
								cx="4"
								cy="6"
								r="1"
							/><circle cx="8" cy="6" r="1" /><circle cx="4" cy="10" r="1" /><circle
								cx="8"
								cy="10"
								r="1"
							/></svg
						>
						{title(tab)}{#if tab.dirty}<span aria-label={$t('workbench.unsaved')}>
								•</span
							>{/if}{#if tab.changed}<span
								class="ml-1 text-blue-600"
								aria-label={$t('workbench.updated')}>●</span
							>{/if}
					</button>
					<button
						type="button"
						aria-label={$t('workbench.close', { values: { title: title(tab) } })}
						class="min-h-[26px] min-w-7 text-slate-500 hover:text-slate-900"
						data-testid="task-tab-close"
						data-tab-key={tab.key}
						draggable="false"
						onclick={() => {
							if (!dragging && Date.now() >= suppressClickUntil) void close(tab.key);
						}}>×</button
					>
				</div>
			{/each}
		</div>
		{#if $workbench.restoredTrimmed}<p role="status" class="px-4 py-2 text-xs text-slate-500">
				{$t('workbench.restoredLimit')}
			</p>{/if}
		{#if $workbench.limit}<p
				role="alert"
				class="px-4 py-2 text-sm text-amber-700"
				data-testid="task-tab-limit"
			>
				{$t('workbench.limit')}
			</p>{/if}
	</div>
{/if}
{#if confirming}
	<div
		class="fixed inset-0 z-[80] flex items-center justify-center bg-black/40 p-4"
		role="presentation"
	>
		<div
			role="dialog"
			aria-modal="true"
			aria-labelledby="task-close-title"
			tabindex="-1"
			class="max-w-md rounded-lg bg-white p-6 shadow-xl dark:bg-slate-900"
			data-testid="task-close-confirm"
		>
			<h2 id="task-close-title" class="text-lg font-semibold">{$t('workbench.closeDirty')}</h2>
			<p class="mt-2 text-sm text-slate-500">{$t('workbench.closeDirtyBody')}</p>
			<div class="mt-5 flex justify-end gap-3">
				<button
					type="button"
					class="min-h-11 rounded border px-4"
					onclick={() => (confirming = null)}>{$t('common.cancel')}</button
				><button
					type="button"
					class="min-h-11 rounded bg-red-600 px-4 text-white"
					onclick={() => close(confirming!, true)}>{$t('workbench.discardClose')}</button
				>
			</div>
		</div>
	</div>
{/if}
