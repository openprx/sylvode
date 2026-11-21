<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '#lib/ui/button/Button.svelte';
	import DestructiveDialog from '#lib/ui/dialog/DestructiveDialog.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import type { Connection } from './wire';

	export type Pending = 'disable' | 'rotate' | 'remove' | null;

	interface Props {
		connection: Connection;
		projectName: string;
		pending: Pending;
		busy: boolean;
		/** The one confirmed action; the dialog stays open while it runs. */
		onconfirm: (action: 'disable' | 'rotate' | 'remove') => void;
		onclose: () => void;
	}
	let { connection, projectName, pending, busy, onconfirm, onclose }: Props = $props();

	const bot = $derived(connection.bot.name);
	const projects = $derived(connection.runner.shared_with_projects.map((p) => p.name).join(', '));
	const others = $derived(
		connection.runner.shared_with_projects
			.filter((project) => project.id !== connection.project_id)
			.map((project) => project.name)
			.join(', ')
	);
	const shared = $derived(others !== '');
</script>

<Dialog
	open={pending === 'disable'}
	title={$t('execution.connection.disableDialog.title', { values: { bot, project: projectName } })}
	role="alertdialog"
	{onclose}
>
	<p>{$t('execution.connection.disableDialog.body')}</p>
	{#if !shared}<p>{$t('execution.connection.disableDialog.bodyLast')}</p>{/if}
	{#snippet actions()}
		<Button variant="secondary" disabled={busy} onclick={onclose}>
			{$t('execution.connection.cancel')}
		</Button>
		<Button variant="primary" loading={busy} onclick={() => onconfirm('disable')}>
			{$t('execution.connection.disableDialog.confirm')}
		</Button>
	{/snippet}
</Dialog>

<DestructiveDialog
	open={pending === 'rotate'}
	title={$t('execution.connection.rotateDialog.title', { values: { bot } })}
	confirmLabel={$t('execution.connection.rotateDialog.confirm')}
	busyLabel={$t('execution.connection.rotateDialog.busy')}
	{busy}
	onconfirm={() => onconfirm('rotate')}
	{onclose}
>
	<p>{$t('execution.connection.rotateDialog.body', { values: { projects } })}</p>
</DestructiveDialog>

<DestructiveDialog
	open={pending === 'remove'}
	title={$t('execution.connection.removeDialog.title', { values: { bot, project: projectName } })}
	confirmLabel={$t('execution.connection.removeDialog.confirm')}
	busyLabel={$t('execution.connection.removeDialog.busy')}
	{busy}
	onconfirm={() => onconfirm('remove')}
	{onclose}
>
	{#if shared}
		<p>{$t('execution.connection.removeDialog.bodyShared', { values: { projects: others } })}</p>
	{:else}
		<p>
			{$t('execution.connection.removeDialog.bodyLast', {
				values: { address: connection.runner.url }
			})}
		</p>
	{/if}
</DestructiveDialog>
