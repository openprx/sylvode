<script lang="ts">
	import { t } from 'svelte-i18n';
	import { fade } from 'svelte/transition';
	import '../foundations/visually-hidden.css';
	import Info from '@lucide/svelte/icons/info';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import X from '@lucide/svelte/icons/x';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { onModalChange } from '../foundations/modal';
	import { portal } from '../foundations/portal';
	import type { Toasts } from './toast-store';
	import { isEditableElement, isRegionKey, isUndoKey, type ToastItem } from './toast-queue';

	interface Props {
		toasts: Toasts;
		/** Drawn inside a frame (absolute) instead of fixed to the window. */
		hosted?: boolean;
	}

	let { toasts, hosted = false }: Props = $props();

	// The toast layer is `--rf-z-toast` from tokens.css.
	const items = $state<{ list: ToastItem[]; live: { text: string; seq: number } }>({
		list: [],
		live: { text: '', seq: 0 }
	});
	let region = $state<HTMLElement | null>(null);
	let returnTo: HTMLElement | null = null;

	// Labels and the announcement phrase are localised here; the store itself holds no strings.
	$effect.pre(() => {
		toasts.setLabels({
			undo: $t('ui.toast.undo'),
			open: $t('ui.toast.open'),
			retry: $t('ui.toast.retry')
		});
		toasts.setFormatter((message, action) =>
			action ? $t('ui.toast.actionAvailable', { values: { message, action } }) : message
		);
	});

	$effect(() => {
		return toasts.subscribe((store) => {
			items.list = store.items;
			items.live = store.announcement;
		});
	});

	// New toasts wait while a dialog is open and appear when the last one closes.
	$effect(() => onModalChange((open) => toasts.holdWhileModal(open)));

	$effect(() => {
		const onVisibility = () => toasts.setDocumentHidden(document.visibilityState === 'hidden');
		document.addEventListener('visibilitychange', onVisibility);
		onVisibility();
		return () => document.removeEventListener('visibilitychange', onVisibility);
	});

	// Keys a page can own: Ctrl/Cmd+Z outside text fields runs the newest Undo, Alt+Shift+N moves
	// focus into the region. Hosted surfaces register neither.
	$effect(() => {
		if (hosted) return;
		function onKeydown(event: KeyboardEvent) {
			if (event.defaultPrevented) return;
			const target = document.activeElement as HTMLElement | null;
			if (isEditableElement(target)) return;
			if (isUndoKey(event)) {
				if (toasts.runNewestUndo()) event.preventDefault();
			} else if (isRegionKey(event)) {
				const cards = region?.querySelectorAll<HTMLElement>('[data-toast]');
				const control = cards?.[cards.length - 1]?.querySelector<HTMLElement>(
					'[data-toast-action], [data-toast-close]'
				);
				if (!control) return;
				event.preventDefault();
				returnTo = target && target !== document.body ? target : null;
				control.focus();
			}
		}
		document.addEventListener('keydown', onKeydown);
		return () => document.removeEventListener('keydown', onKeydown);
	});

	function tokenMs(name: string): number {
		const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
		const value = Number.parseFloat(raw);
		return Number.isFinite(value) ? value : 0;
	}

	function restoreFocus() {
		const target = returnTo?.isConnected ? returnTo : document.querySelector<HTMLElement>('main');
		returnTo = null;
		if (!target) return;
		if (!target.hasAttribute('tabindex') && target.tagName === 'MAIN') target.tabIndex = -1;
		target.focus();
	}

	function onCardKeydown(event: KeyboardEvent, id: string) {
		if (event.key !== 'Escape') return;
		event.preventDefault();
		event.stopPropagation();
		toasts.dismiss(id);
		restoreFocus();
	}
</script>

<section
	bind:this={region}
	class="ui-toast-region"
	class:hosted
	aria-label={$t('ui.toast.region')}
	data-ui="toast-region"
	use:portal
>
	<div class="ui-sr-only" role="status" aria-live="polite" aria-atomic="true" data-toast-live>
		{#key items.live.seq}{items.live.text}{/key}
	</div>
	{#each items.list as item (item.id)}
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="card"
			role="group"
			aria-label={$t('ui.toast.notification')}
			data-toast={item.id}
			data-tone={item.tone}
			in:fade={{ duration: tokenMs('--rf-duration-base') }}
			out:fade={{ duration: tokenMs('--rf-duration-base') }}
			onpointerenter={() => toasts.hold(item.id, 'hover')}
			onpointerleave={() => toasts.release(item.id, 'hover')}
			onfocusin={() => toasts.hold(item.id, 'focus')}
			onfocusout={() => toasts.release(item.id, 'focus')}
			onkeydown={(event) => onCardKeydown(event, item.id)}
		>
			<span class="glyph tone-{item.tone}">
				{#if item.tone === 'success'}
					<CircleCheck size={16} aria-hidden="true" />
				{:else if item.tone === 'warning'}
					<TriangleAlert size={16} aria-hidden="true" />
				{:else if item.action?.kind === 'undo'}
					<Trash2 size={16} aria-hidden="true" />
				{:else}
					<Info size={16} aria-hidden="true" />
				{/if}
			</span>
			<p class="message">{item.message}</p>
			{#if item.action}
				<button
					type="button"
					class="action"
					data-toast-action
					aria-disabled={item.busy || undefined}
					onclick={() => !item.busy && void toasts.run(item.id)}
				>
					{#if item.busy}
						<LoaderCircle size={14} class="spin" aria-hidden="true" />
						{$t('ui.toast.restoring')}
					{:else}
						{item.action.label}
					{/if}
				</button>
			{/if}
			<button
				type="button"
				class="close"
				data-toast-close
				aria-label={$t('ui.toast.dismiss')}
				onclick={() => toasts.dismiss(item.id)}
			>
				<X size={14} aria-hidden="true" />
			</button>
		</div>
	{/each}
</section>

<style>
	.ui-toast-region {
		position: fixed;
		z-index: var(--rf-z-toast);
		inset-inline-end: 24px;
		bottom: 24px;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		width: 360px;
		pointer-events: none;
		font-family: var(--rf-font-sans-stack);
	}
	.ui-toast-region.hosted {
		position: absolute;
	}
	.card {
		pointer-events: auto;
		box-sizing: border-box;
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-5);
		padding: var(--rf-space-5) var(--rf-space-6);
		border: var(--rf-border-width) solid var(--rf-overlay-border);
		border-radius: var(--rf-radius-menu);
		background: var(--rf-overlay);
		box-shadow: var(--rf-shadow-overlay);
		color: var(--rf-text);
	}
	.glyph {
		display: inline-grid;
		margin-top: 1px;
		color: var(--rf-secondary);
	}
	.tone-success {
		color: var(--rf-success);
	}
	.tone-warning {
		color: var(--rf-warning);
	}
	.message {
		flex: 1;
		min-width: 0;
		margin: 0;
		font-size: var(--rf-size-secondary);
		line-height: 1.45;
		font-weight: var(--rf-weight-regular);
		overflow-wrap: anywhere;
	}
	.action,
	.close {
		flex: none;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--rf-space-3);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		cursor: pointer;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.action {
		height: 24px;
		padding: 0 var(--rf-space-4);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
	}
	.close {
		width: 24px;
		height: 24px;
		padding: 0;
		color: var(--rf-secondary);
	}
	.action:hover,
	.close:hover {
		background: var(--rf-hover);
		color: var(--rf-text);
	}
	.action:active,
	.close:active {
		background: var(--rf-pressed);
	}
	.action:focus-visible,
	.close:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.action[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: progress;
	}
	.action :global(.spin) {
		animation: ui-toast-spin 900ms linear infinite;
	}
	@keyframes ui-toast-spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (max-width: 479px) {
		.ui-toast-region {
			inset-inline: 16px;
			bottom: 16px;
			width: auto;
		}
	}
	.ui-toast-region.hosted {
		inset-inline-end: 16px;
		bottom: 16px;
		width: min(360px, calc(100% - 32px));
	}
	@media (pointer: coarse) {
		.action,
		.close {
			position: relative;
		}
		.action::after,
		.close::after {
			content: '';
			position: absolute;
			inset: -10px;
		}
	}
</style>
