<svelte:options runes />

<script lang="ts" module>
	export type PendingAction = 'pause' | 'rotate' | 'takeOver' | 'delete' | null;
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import DestructiveDialog from '#lib/ui/dialog/DestructiveDialog.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import { ruleNames } from './source-view';
	import type { Rule, Source } from './wire';

	interface Props {
		source: Source;
		pending: PendingAction;
		/** The rules the delete takes with it (the source's bound rules, read when it was asked). */
		bound: Rule[];
		busy: boolean;
		/** The one confirmed action; the dialog stays open while it runs. */
		onconfirm: (action: Exclude<PendingAction, null>) => void;
		onclose: () => void;
	}
	let { source, pending, bound, busy, onconfirm, onclose }: Props = $props();

	const name = $derived(source.name);
</script>

<Dialog
	open={pending === 'pause'}
	title={$t('inbound.sources.pauseDialog.title', { values: { source: name } })}
	role="alertdialog"
	{onclose}
>
	<p>{$t('inbound.sources.pauseDialog.body')}</p>
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} data-autofocus onclick={onclose}>
			{$t('inbound.sources.cancel')}
		</Button>
		<Button variant="primary" loading={busy} onclick={() => onconfirm('pause')}>
			{$t('inbound.sources.pauseDialog.confirm')}
		</Button>
	{/snippet}
</Dialog>

<Dialog
	open={pending === 'takeOver'}
	title={$t('inbound.sources.takeOverDialog.title', { values: { source: name } })}
	role="alertdialog"
	{onclose}
>
	<p>{$t('inbound.sources.takeOverDialog.body')}</p>
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} data-autofocus onclick={onclose}>
			{$t('inbound.sources.cancel')}
		</Button>
		<Button variant="primary" loading={busy} onclick={() => onconfirm('takeOver')}>
			{$t('inbound.sources.takeOverDialog.confirm')}
		</Button>
	{/snippet}
</Dialog>

<DestructiveDialog
	open={pending === 'rotate'}
	title={$t('inbound.sources.rotateDialog.title', { values: { source: name } })}
	confirmLabel={$t('inbound.sources.rotateDialog.confirm')}
	busyLabel={$t('inbound.sources.rotateDialog.busy')}
	{busy}
	onconfirm={() => onconfirm('rotate')}
	{onclose}
>
	<p>{$t('inbound.sources.rotateDialog.body', { values: { source: name } })}</p>
</DestructiveDialog>

<DestructiveDialog
	open={pending === 'delete'}
	title={$t('inbound.sources.deleteDialog.title', { values: { source: name } })}
	confirmLabel={$t('inbound.sources.deleteDialog.confirm')}
	busyLabel={$t('inbound.sources.deleteDialog.busy')}
	{busy}
	onconfirm={() => onconfirm('delete')}
	{onclose}
>
	<p data-testid="delete-source-body">
		{$t('inbound.sources.deleteDialog.body')}
	</p>
	{#if bound.length > 0}
		<p data-testid="delete-source-rules">
			{$t('inbound.sources.deleteDialog.bodyRules', {
				values: { count: bound.length, rules: ruleNames(bound) }
			})}
		</p>
	{/if}
</DestructiveDialog>
