<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import Button from '../button/Button.svelte';
	import Dialog from './Dialog.svelte';

	interface Props {
		open: boolean;
		/** A question that names the object ("Delete project Alpha?"). */
		title: string;
		/** Confirm label with verb and object ("Delete project"). Never "OK". */
		confirmLabel: string;
		onconfirm: () => void;
		onclose: () => void;
		/** Progress verb while the confirmed action runs. */
		busyLabel?: string;
		/** Defaults to the shared "Cancel". */
		cancelLabel?: string;
		busy?: boolean;
		testId?: string;
		/** What happens, what is kept, and that it cannot be undone. */
		children: Snippet;
	}

	let {
		open,
		title,
		confirmLabel,
		onconfirm,
		onclose,
		busyLabel,
		cancelLabel,
		busy = false,
		testId,
		children
	}: Props = $props();
</script>

<Dialog {open} {title} {onclose} {testId} role="alertdialog" {children}>
	{#snippet actions()}
		<!-- Focus starts on Cancel, so Enter never confirms a loss by accident. -->
		<Button variant="secondary" data-autofocus onclick={() => onclose()}>
			{cancelLabel ?? $t('ui.dialog.cancel')}
		</Button>
		<Button variant="danger" loading={busy} loadingLabel={busyLabel} onclick={() => onconfirm()}>
			{confirmLabel}
		</Button>
	{/snippet}
</Dialog>
