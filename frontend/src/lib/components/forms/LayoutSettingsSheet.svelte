<script lang="ts">
	/**
	 * The "Layout settings" sheet: a non-modal side sheet at the inline end that hosts the
	 * advanced view controls (view type, per-type parameters, filter groups, columns). Controls
	 * apply live, so the sheet has no save button, only Done.
	 */
	import { tick, type Snippet } from 'svelte';
	import { t } from 'svelte-i18n';
	import { X } from '@lucide/svelte';

	interface Props {
		open: boolean;
		viewName: string;
		onclose: () => void;
		children: Snippet;
	}

	let { open, viewName, onclose, children }: Props = $props();

	let heading = $state<HTMLElement | null>(null);
	let returnFocus: HTMLElement | null = null;
	let wasOpen = false;

	$effect(() => {
		if (open && !wasOpen) {
			returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
			void tick().then(() => heading?.focus());
		}
		if (!open && wasOpen) returnFocus?.focus();
		wasOpen = open;
	});

	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape' || event.defaultPrevented) return;
		const target = event.target as HTMLElement | null;
		// Esc inside a native select or an open disclosure belongs to that control.
		if (target?.tagName === 'SELECT') return;
		event.preventDefault();
		onclose();
	}
</script>

{#if open}
	<!-- Esc from any control inside the sheet closes it; the handler only listens. -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<aside
		class="layout-sheet fixed end-4 top-4 bottom-4 z-40 flex w-[min(560px,calc(100vw-2rem))] flex-col rounded-lg border border-slate-200 bg-white shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:ring-1 dark:ring-slate-700"
		aria-labelledby="layout-settings-title"
		data-layout-settings-sheet
		onkeydown={onKeydown}
	>
		<header
			class="flex items-center justify-between gap-3 border-b border-slate-200 px-4 py-3 dark:border-slate-700"
		>
			<h2
				id="layout-settings-title"
				tabindex="-1"
				bind:this={heading}
				class="min-w-0 truncate text-base font-semibold text-slate-950 focus:outline-none dark:text-slate-100"
			>
				{$t('forms.views.layoutSettingsTitle', { values: { name: viewName } })}
			</h2>
			<button
				type="button"
				class="inline-flex h-8 w-8 items-center justify-center rounded-md text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
				aria-label={$t('common.close')}
				onclick={onclose}><X class="h-4 w-4" aria-hidden="true" /></button
			>
		</header>
		<div class="min-h-0 flex-1 overflow-y-auto p-3">
			{@render children()}
		</div>
		<footer
			class="flex flex-wrap items-center justify-between gap-3 border-t border-slate-200 px-4 py-3 dark:border-slate-700"
		>
			<p class="text-xs text-slate-500 dark:text-slate-400">{$t('forms.views.layoutHint')}</p>
			<button
				type="button"
				class="h-8 rounded-md bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700"
				data-layout-settings-done
				onclick={onclose}>{$t('forms.views.done')}</button
			>
		</footer>
	</aside>
{/if}

<style>
	.layout-sheet {
		animation: sheet-in 180ms cubic-bezier(0.2, 0, 0, 1);
	}

	@keyframes sheet-in {
		from {
			opacity: 0;
			transform: translateX(16px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.layout-sheet {
			animation: sheet-fade 80ms linear;
		}

		@keyframes sheet-fade {
			from {
				opacity: 0;
			}
			to {
				opacity: 1;
			}
		}
	}
</style>
