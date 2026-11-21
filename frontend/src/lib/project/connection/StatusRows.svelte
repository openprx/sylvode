<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { componentChip, deliveryChip, deliveryValue, relativeTime, runnerChip } from './status';
	import type { Cli, Status } from './wire';

	interface Props {
		status: Status | null;
		cli: Cli;
		/** The channel of the connection: tmux is shown only for `tmux`. */
		showTmux: boolean;
		now: Date;
		refreshing?: boolean;
		onrefresh: () => void;
	}
	let { status, cli, showTmux, now, refreshing = false, onrefresh }: Props = $props();

	const language = $derived($locale ?? 'en');
	const runner = $derived(status ? runnerChip(status.runner.state) : null);
	const runnerValue = $derived.by(() => {
		if (!status) return '';
		const { state, reported_at, version } = status.runner;
		if (state === 'waiting') return $t('execution.connection.status.runner.waitingHint');
		if (reported_at === null) return $t('execution.connection.status.runner.noReport');
		return $t('execution.connection.status.runner.lastReport', {
			values: { time: relativeTime(reported_at, now, language), version: version ?? '?' }
		});
	});
</script>

<div class="status" data-testid="connection-status">
	<div class="head">
		<IconButton
			icon={RefreshCw}
			label={$t('execution.connection.status.refresh')}
			disabled={refreshing}
			onclick={onrefresh}
		/>
	</div>
	{#if status && runner}
		<SettingRow label={$t('execution.connection.status.runner.label')} value={runnerValue}>
			{#snippet chip()}
				<StatusChip tone={runner.tone} data-testid="runner-chip">{$t(runner.key)}</StatusChip>
			{/snippet}
		</SettingRow>
		{#if status.runner.warning === 'protocol_mismatch'}
			<p class="hint">{$t('execution.connection.status.runner.protocolMismatch')}</p>
		{/if}
		{@const cliChip = componentChip(status.cli.state)}
		<SettingRow
			label={$t('execution.connection.status.cli', {
				values: { name: $t(`execution.connection.cli.${cli}`) }
			})}
			value={status.cli.version ?? ''}
		>
			{#snippet chip()}
				<StatusChip tone={cliChip.tone} data-testid="cli-chip">{$t(cliChip.key)}</StatusChip>
			{/snippet}
		</SettingRow>
		{#if showTmux && status.tmux}
			{@const tmuxChip = componentChip(status.tmux.state)}
			<SettingRow label={$t('execution.connection.status.tmux')} value={status.tmux.version ?? ''}>
				{#snippet chip()}
					<StatusChip tone={tmuxChip.tone} data-testid="tmux-chip">{$t(tmuxChip.key)}</StatusChip>
				{/snippet}
			</SettingRow>
		{/if}
		{@const delivery = deliveryChip(status.last_delivery.state)}
		<SettingRow
			label={$t('execution.connection.status.delivery.label')}
			value={deliveryValue(status.last_delivery, now, language)}
		>
			{#snippet chip()}
				<StatusChip tone={delivery.tone} data-testid="delivery-chip">{$t(delivery.key)}</StatusChip>
			{/snippet}
		</SettingRow>
	{/if}
</div>

<style>
	.status {
		display: flex;
		flex-direction: column;
	}
	.head {
		display: flex;
		justify-content: flex-end;
	}
	.hint {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
</style>
