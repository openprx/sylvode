<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../button/Button.svelte';
	import TextField from '../text-field/TextField.svelte';
	import Dialog from './Dialog.svelte';
	import DestructiveDialog from './DestructiveDialog.svelte';
	import ConfirmHost from './ConfirmHost.svelte';
	import { confirmAction } from './confirm';

	let open = $state(false);
	let destructiveOpen = $state(false);
	let draft = $state('');
	let nestedOpen = $state(false);
	let outcome = $state<'none' | 'yes' | 'no'>('none');

	async function ask(): Promise<void> {
		const ok = await confirmAction({
			title: $t('ui.confirm.gallery.title'),
			body: $t('ui.confirm.gallery.body'),
			verb: 'delete',
			object: $t('ui.confirm.gallery.object')
		});
		outcome = ok ? 'yes' : 'no';
	}

	// Two calls in a row: the second dialog opens only after the first is answered.
	async function askTwice(): Promise<void> {
		const first = ask();
		const second = confirmAction({
			title: $t('ui.confirm.gallery.second'),
			verb: 'remove',
			object: $t('ui.confirm.gallery.secondObject')
		});
		await first;
		await second;
	}

	async function askNeutral(): Promise<void> {
		const ok = await confirmAction({
			tone: 'neutral',
			title: $t('ui.confirm.gallery.neutralTitle'),
			confirmLabel: $t('ui.confirm.gallery.neutralLabel')
		});
		outcome = ok ? 'yes' : 'no';
	}
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

<section data-gallery-section="confirm">
	<h2>confirmAction</h2>
	<div class="row">
		<span data-gallery="confirm:destructive">
			<Button variant="danger-quiet" data-testid="confirm-opener" onclick={ask}>
				{$t('ui.confirm.gallery.open')}
			</Button>
		</span>
		<span data-gallery="confirm:twice">
			<Button data-testid="confirm-twice-opener" onclick={askTwice}>
				{$t('ui.confirm.gallery.openTwo')}
			</Button>
		</span>
		<span data-gallery="confirm:neutral">
			<Button data-testid="confirm-neutral-opener" onclick={askNeutral}>
				{$t('ui.confirm.gallery.openNeutral')}
			</Button>
		</span>
		<span data-gallery="confirm:nested">
			<Button data-testid="confirm-nested-opener" onclick={() => (nestedOpen = true)}>
				{$t('ui.confirm.gallery.openNested')}
			</Button>
		</span>
	</div>
	<p data-testid="confirm-result" data-outcome={outcome}>
		{$t('ui.confirm.gallery.result', { values: { value: $t(`ui.confirm.gallery.${outcome}`) } })}
	</p>
</section>

<ConfirmHost />

<Dialog
	open={nestedOpen}
	title={$t('ui.confirm.gallery.nestedTitle')}
	onclose={() => (nestedOpen = false)}
	testId="confirm-nested-dialog"
>
	<Button data-testid="confirm-nested-ask" onclick={ask}>{$t('ui.confirm.gallery.ask')}</Button>
</Dialog>

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
