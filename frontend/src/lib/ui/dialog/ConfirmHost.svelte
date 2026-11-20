<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import { afterNavigate } from '$app/navigation';
	import Button from '../button/Button.svelte';
	import Dialog from './Dialog.svelte';
	import DestructiveDialog from './DestructiveDialog.svelte';
	import {
		confirmLabelOf,
		confirmQueue,
		type ConfirmHostHandle,
		type ConfirmRequest
	} from './confirm';

	/**
	 * Renders the requests made with `confirmAction()`. Mount it once, in the app shell. A second
	 * mounted host stays inert, so mounting it in a test page as well is safe.
	 */
	let shown = $state<ConfirmRequest | null>(null);
	let handle: ConfirmHostHandle | null = null;
	// True between two requests: the first dialog must leave the DOM (and give focus back to the
	// caller) before the next one captures the focus to restore.
	let gap = false;

	function translate(key: string, values?: Record<string, string>): string {
		return $t(key, { values });
	}

	function sync(): void {
		if (gap) return;
		const head = handle?.isActive() ? confirmQueue.current() : null;
		if (head === shown) return;
		if (shown !== null) {
			shown = null;
			gap = true;
			void tick().then(() =>
				queueMicrotask(() => {
					gap = false;
					sync();
				})
			);
			return;
		}
		shown = head;
	}

	function answer(result: boolean): void {
		if (shown) confirmQueue.settle(shown, result);
	}

	onMount(() => {
		handle = confirmQueue.attachHost();
		const stop = confirmQueue.subscribe(sync);
		sync();
		return () => {
			stop();
			handle?.detach();
			handle = null;
			shown = null;
		};
	});

	// The page that asked is gone: decline its requests instead of leaving a dialog for it.
	afterNavigate(() => confirmQueue.cancelOutside(location.pathname));
</script>

{#if shown}
	{@const options = shown.options}
	{@const confirmLabel = confirmLabelOf(options, translate)}
	{@const cancelLabel = options.cancelLabel ?? $t('ui.dialog.cancel')}
	{#if options.tone === 'neutral'}
		<Dialog
			open
			role="alertdialog"
			title={options.title}
			onclose={() => answer(false)}
			testId="confirm-dialog"
		>
			{#if options.body}{options.body}{/if}
			{#snippet actions()}
				<Button variant="secondary" data-autofocus onclick={() => answer(false)}>
					{cancelLabel}
				</Button>
				<Button
					variant="primary"
					title={confirmLabel}
					aria-label={confirmLabel}
					onclick={() => answer(true)}
				>
					<span class="confirm-label">{confirmLabel}</span>
				</Button>
			{/snippet}
		</Dialog>
	{:else}
		<DestructiveDialog
			open
			title={options.title}
			{confirmLabel}
			{cancelLabel}
			onconfirm={() => answer(true)}
			onclose={() => answer(false)}
			testId="confirm-dialog"
		>
			{#if options.body}{options.body}{/if}
		</DestructiveDialog>
	{/if}
{/if}

<style>
	.confirm-label {
		display: block;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
