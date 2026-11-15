<script lang="ts">
	import { t } from 'svelte-i18n';
	import { getContext } from 'svelte';
	import { TASK_EDIT_SURFACE, taskEditorAction } from '#lib/workbench/edit-surface.js';
	interface Props {
		open?: boolean;
		taskScoped?: boolean;
		title: string;
		maxWidthClass?: string;
		onclose?: () => void;
		children?: import('svelte').Snippet;
		footer?: import('svelte').Snippet;
	}

	let {
		open = $bindable(false),
		taskScoped = false,
		title,
		maxWidthClass = 'max-w-md',
		onclose,
		children,
		footer
	}: Props = $props();

	const editorAction = taskEditorAction();
	const hasSurface = !!getContext(TASK_EDIT_SURFACE);
	const scoped = $derived(taskScoped && hasSurface);
	function handleBackdropClick(_e: MouseEvent) {
		// Do not close on backdrop click — require explicit close (X or Cancel)
	}

	function close() {
		open = false;
		onclose?.();
	}

	function handleEscape(_e: KeyboardEvent) {
		// Do not close on Escape — require explicit close (X or Cancel)
	}
</script>

<svelte:window onkeydown={handleEscape} />

{#if open}
	<div
		use:editorAction={scoped}
		class="{scoped
			? 'pointer-events-auto absolute inset-0 z-30'
			: 'fixed inset-0 z-50'} flex items-center justify-center bg-black/50 p-4 dark:bg-black/70"
		onclick={handleBackdropClick}
		onkeydown={handleEscape}
		role="dialog"
		tabindex="-1"
		aria-modal={scoped ? 'false' : 'true'}
		aria-labelledby="modal-title"
	>
		<div
			class={`w-full max-h-[90vh] overflow-y-auto rounded-lg bg-white shadow-xl dark:bg-slate-800 dark:shadow-slate-900/50 ${maxWidthClass}`}
		>
			<!-- Header -->
			<div
				class="flex items-center justify-between border-b border-slate-200 p-6 dark:border-slate-700"
			>
				<h2 id="modal-title" class="text-xl font-semibold text-slate-900 dark:text-slate-100">
					{title}
				</h2>
				<button
					onclick={close}
					class="text-slate-400 hover:text-slate-600 dark:text-slate-300 focus:outline-none dark:hover:text-slate-200"
					aria-label={$t('common.close')}
				>
					<svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							stroke-width="2"
							d="M6 18L18 6M6 6l12 12"
						></path>
					</svg>
				</button>
			</div>

			<!-- Body -->
			<div class="p-6">
				{#if children}
					{@render children()}
				{/if}
			</div>

			<!-- Footer -->
			{#if footer}
				<div
					class="flex items-center justify-end gap-3 border-t border-slate-200 p-6 dark:border-slate-700"
				>
					{@render footer()}
				</div>
			{/if}
		</div>
	</div>
{/if}
