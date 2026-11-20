<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import TextField from '../text-field/TextField.svelte';
	import Dialog from './Dialog.svelte';
	import DestructiveDialog from './DestructiveDialog.svelte';

	let open = $state(false);
	let destructiveOpen = $state(false);
	let draft = $state('');
</script>

<section data-gallery-section="dialog">
	<h2>dialog</h2>
	<div class="row">
		<span data-gallery="dialog:open-button">
			<Button data-testid="dialog-opener" onclick={() => (open = true)}>
				{$t('ui.gallery.open')}
			</Button>
		</span>
		<span data-gallery="dialog:destructive-open-button">
			<Button
				variant="danger-quiet"
				data-testid="destructive-opener"
				onclick={() => (destructiveOpen = true)}
			>
				{$t('ui.gallery.confirm')}
			</Button>
		</span>
	</div>
</section>

<!-- The draft lives here, outside the dialog, so Escape keeps it. -->
<Dialog {open} title={$t('ui.gallery.title')} onclose={() => (open = false)} testId="dialog-demo">
	<TextField label={$t('ui.gallery.field')} bind:value={draft} />
	{#snippet actions()}
		<Button onclick={() => (open = false)}>{$t('ui.dialog.cancel')}</Button>
		<Button variant="primary" onclick={() => (open = false)}>{$t('ui.gallery.sample')}</Button>
	{/snippet}
</Dialog>

<DestructiveDialog
	open={destructiveOpen}
	title={$t('ui.gallery.destructiveTitle')}
	confirmLabel={$t('ui.gallery.confirm')}
	onconfirm={() => (destructiveOpen = false)}
	onclose={() => (destructiveOpen = false)}
	testId="destructive-demo"
>
	{$t('ui.gallery.destructiveBody')}
</DestructiveDialog>

<style>
	.row {
		display: flex;
		gap: var(--rf-space-4);
	}
</style>
