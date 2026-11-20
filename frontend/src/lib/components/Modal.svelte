<script module lang="ts">
	/**
	 * Tailwind widths up to `max-w-lg` (512) fit the primitive's 480 dialog; anything wider uses the
	 * 680 one, the widest the primitive has.
	 */
	const NARROW = new Set(['max-w-xs', 'max-w-sm', 'max-w-md', 'max-w-lg']);

	export function dialogSize(maxWidthClass: string): 'md' | 'wide' {
		return NARROW.has(maxWidthClass) ? 'md' : 'wide';
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { getContext, type Snippet } from 'svelte';
	import X from '@lucide/svelte/icons/x';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import { captureFocus } from '#lib/ui/foundations/focus.js';
	import { focusTrap } from '#lib/ui/foundations/focus-trap.js';
	import { uid } from '#lib/ui/foundations/ids.js';
	import { pushLayer } from '#lib/ui/foundations/layers.js';
	import { enterModal } from '#lib/ui/foundations/modal.js';
	import { TASK_EDIT_SURFACE, taskEditorAction } from '#lib/workbench/edit-surface.js';

	interface Props {
		open?: boolean;
		taskScoped?: boolean;
		title: string;
		maxWidthClass?: string;
		onclose?: () => void;
		children?: Snippet;
		footer?: Snippet;
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

	// Escape and the close button end the dialog; the page's own state (the draft) is untouched.
	// A backdrop click does nothing.
	function close() {
		open = false;
		onclose?.();
	}

	// Task-scoped dialogs live inside the task panel's overlay, which the primitive cannot do, so
	// they apply the same foundations (layer, focus trap, focus return, toast hold) themselves.
	const titleId = uid('modal-title');
	let restoreFocus: (() => void) | null = null;
	let wasOpen = false;

	$effect.pre(() => {
		if (scoped && open && !wasOpen) restoreFocus = captureFocus();
		wasOpen = open;
	});

	$effect(() => {
		if (!scoped || !open) return;
		const stopLayer = pushLayer({ onEscape: close });
		const leaveModal = enterModal();
		return () => {
			stopLayer();
			leaveModal();
			const restore = restoreFocus;
			restoreFocus = null;
			queueMicrotask(() => restore?.());
		};
	});
</script>

{#if scoped}
	{#if open}
		<div use:editorAction={true} class="ui-modal-scrim" data-ui-layer="dialog">
			<div
				class="ui-modal size-{dialogSize(maxWidthClass)}"
				role="dialog"
				aria-modal="false"
				aria-labelledby={titleId}
				tabindex="-1"
				use:focusTrap={{ active: true }}
			>
				<header>
					<h2 id={titleId}>{title}</h2>
					<IconButton icon={X} label={$t('common.close')} onclick={close} />
				</header>
				<div class="body">{@render children?.()}</div>
				{#if footer}<footer>{@render footer()}</footer>{/if}
			</div>
		</div>
	{/if}
{:else}
	<Dialog {open} {title} size={dialogSize(maxWidthClass)} onclose={close} actions={footer}>
		{@render children?.()}
	</Dialog>
{/if}

<style>
	.ui-modal-scrim {
		position: absolute;
		inset: 0;
		z-index: var(--rf-z-panel);
		display: grid;
		place-items: center;
		padding: var(--rf-space-9);
		background: var(--rf-scrim);
		pointer-events: auto;
	}
	.ui-modal {
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
	.ui-modal:focus-visible {
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
	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--rf-space-4);
	}
</style>
