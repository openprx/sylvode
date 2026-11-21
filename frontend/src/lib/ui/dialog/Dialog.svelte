<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import X from '@lucide/svelte/icons/x';
	import IconButton from '../icon-button/IconButton.svelte';
	import { captureFocus } from '../foundations/focus';
	import { focusTrap } from '../foundations/focus-trap';
	import { uid } from '../foundations/ids';
	import { pushLayer } from '../foundations/layers';
	import { enterModal } from '../foundations/modal';
	import { portal } from '../foundations/portal';
	import { lockScroll } from '../foundations/scroll-lock';

	interface Props {
		open: boolean;
		title: string;
		/** Called for Escape and the header close button. A backdrop click does nothing. */
		onclose: () => void;
		/** 480 wide by default; the task-detail dialog is `wide` (680). */
		size?: 'md' | 'wide';
		/** Overrides `size` with a fixed width in px: 560 is the command palette. */
		width?: 560;
		/** No visible header and no padding: the title stays as a visually hidden heading. */
		headless?: boolean;
		/** Keep the content mounted while closed, so local draft state survives Escape. */
		keepMounted?: boolean;
		/** Selector for the element that first receives focus; default is the first field. */
		initialFocus?: string;
		testId?: string;
		/**
		 * `alertdialog` for an interruption that needs an answer (a confirmation): the surface is
		 * then also described by the body text. Default `dialog`.
		 */
		role?: 'dialog' | 'alertdialog';
		children: Snippet;
		/** Right-aligned action row: Cancel first, the confirming action last. */
		actions?: Snippet;
	}

	let {
		open,
		title,
		onclose,
		size = 'md',
		width,
		headless = false,
		keepMounted = false,
		initialFocus,
		testId,
		role = 'dialog',
		children,
		actions
	}: Props = $props();

	const titleId = uid('dialog-title');
	const bodyId = uid('dialog-body');
	let restoreFocus: (() => void) | null = null;
	let wasOpen = false;

	// Runs before the DOM updates, so the opener still has focus when it is remembered.
	$effect.pre(() => {
		if (open && !wasOpen) restoreFocus = captureFocus();
		wasOpen = open;
	});

	$effect(() => {
		if (!open) return;
		const stopLayer = pushLayer({ onEscape: () => onclose() });
		const unlock = lockScroll();
		// New toasts wait while the dialog is open (toast design: nothing appears behind a scrim).
		const leaveModal = enterModal();
		return () => {
			stopLayer();
			unlock();
			leaveModal();
			const restore = restoreFocus;
			restoreFocus = null;
			// After the dialog has left the DOM, so the opener is focusable again.
			queueMicrotask(() => restore?.());
		};
	});
</script>

{#if open || keepMounted}
	<div class="ui-scrim" hidden={!open} data-ui-layer="dialog" use:portal>
		<div
			class="ui-dialog size-{size}"
			class:width-560={width === 560}
			class:headless
			{role}
			aria-modal="true"
			aria-labelledby={titleId}
			aria-describedby={role === 'alertdialog' ? bodyId : undefined}
			tabindex="-1"
			data-testid={testId}
			use:focusTrap={{ active: open, initialFocus }}
		>
			<header>
				<h2 id={titleId}>{title}</h2>
				<IconButton icon={X} label={$t('ui.dialog.close')} onclick={() => onclose()} />
			</header>
			<div class="body" id={bodyId}>{@render children()}</div>
			{#if actions}<footer>{@render actions()}</footer>{/if}
		</div>
	</div>
{/if}

<style>
	.ui-scrim {
		position: fixed;
		inset: 0;
		z-index: var(--rf-z-scrim);
		display: grid;
		/* The track is the scrim's width, never the dialog's content width. */
		grid-template-columns: minmax(0, 1fr);
		place-items: center;
		padding: var(--rf-space-9);
		background: var(--rf-scrim);
	}
	.ui-scrim[hidden] {
		display: none;
	}
	.ui-dialog {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		box-sizing: border-box;
		width: var(--rf-layout-dialog);
		max-width: 100%;
		max-height: 100%;
		overflow: auto;
		padding: var(--rf-space-8);
		border: var(--rf-border-width) solid var(--rf-overlay-border);
		border-radius: var(--rf-radius-dialog);
		background: var(--rf-overlay);
		color: var(--rf-text);
		box-shadow: var(--rf-shadow-overlay);
		font-family: var(--rf-font-sans-stack);
	}
	.size-wide {
		width: 680px;
	}
	.width-560 {
		width: 560px;
	}
	.headless {
		gap: 0;
		padding: 0;
		overflow: hidden;
	}
	.headless > header {
		position: absolute;
		width: 1px;
		height: 1px;
		margin: -1px;
		overflow: hidden;
		clip-path: inset(50%);
	}
	.headless > .body {
		padding: 0;
	}
	.ui-dialog:focus-visible {
		outline: none;
	}
	header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	h2 {
		margin: 0;
		font-size: var(--rf-size-title);
		line-height: var(--rf-leading-title);
		font-weight: var(--rf-weight-semibold);
	}
	.body {
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
	}
	/* Buttons that do not fit side by side wrap onto a second row instead of overlapping. */
	footer {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--rf-space-4);
	}
	footer > :global(.ui-button-wrap) {
		flex: 0 1 auto;
		min-width: 0;
		max-width: 100%;
	}
</style>
