<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../../ui/button/Button.svelte';
	import SettingRow from '../../ui/setting-row/SettingRow.svelte';
	import StateBlock from '../../ui/state-block/StateBlock.svelte';
	import StatusChip from '../../ui/status-chip/StatusChip.svelte';
	import { tooltip } from '../../ui/tooltip/tooltip';
	import { absoluteTime, relativeTime } from '../agent-tasks/time';
	import type { IssueAlert } from './api';
	import { KIND_MAX, NAME_MAX, alertRulesHref, clampText, severityView, taskRowView } from './view';

	interface Props {
		/** The facts, or null when the work item is not an alert or the load failed. */
		alert: IssueAlert | null;
		failed: boolean;
		loading: boolean;
		workspaceId: string;
		projectId: string;
		/** Owner or admin of the workspace: the rule name links to the Alert rules section. */
		isAdmin: boolean;
		onrefresh: () => void;
	}

	let { alert, failed, loading, workspaceId, projectId, isAdmin, onrefresh }: Props = $props();

	const severity = $derived(alert ? severityView(alert.severity) : null);
	const task = $derived(alert ? taskRowView(alert) : null);
</script>

{#if alert || failed}
	<section class="panel" data-testid="alert-panel" aria-labelledby="alert-panel-title">
		<header class="head">
			<div class="who">
				<h2 id="alert-panel-title" class="title">{$t('alertPanel.title')}</h2>
				{#if alert && severity}
					<StatusChip tone={severity.tone}>{$t(severity.labelKey)}</StatusChip>
					<StatusChip>{alert.open ? $t('alertPanel.open') : $t('alertPanel.closed')}</StatusChip>
				{/if}
			</div>
			<Button size="sm" loading={loading && alert !== null} onclick={onrefresh}>
				{$t('alertPanel.refresh')}
			</Button>
		</header>

		{#if failed && !alert}
			<StateBlock kind="loadFailed" thing={$t('alertPanel.thing')} action={{ run: onrefresh }} />
		{:else if alert}
			<div class="rows" data-testid="alert-panel-rows">
				<SettingRow label={$t('alertPanel.source')}>
					{#snippet chip()}
						<span class="text" class:muted={!alert?.source} data-testid="alert-source">
							{alert?.source
								? clampText(alert.source.name, NAME_MAX)
								: $t('alertPanel.deletedSource')}
						</span>
					{/snippet}
				</SettingRow>

				{#if alert.kind}
					<SettingRow label={$t('alertPanel.kind')}>
						{#snippet chip()}
							<span class="mono" data-testid="alert-kind"
								>{clampText(alert?.kind ?? '', KIND_MAX)}</span
							>
						{/snippet}
					</SettingRow>
				{/if}

				<SettingRow label={$t('alertPanel.occurrences')} value={String(alert.occurrences)} />

				<SettingRow label={$t('alertPanel.firstSeen')}>
					{#snippet chip()}
						<time
							class="text"
							datetime={alert?.first_seen_at}
							use:tooltip={{ text: absoluteTime(alert?.first_seen_at ?? ''), describes: true }}
						>
							{relativeTime(alert?.first_seen_at ?? '')}
						</time>
					{/snippet}
				</SettingRow>

				<SettingRow label={$t('alertPanel.lastSeen')}>
					{#snippet chip()}
						<time
							class="text"
							datetime={alert?.last_seen_at}
							use:tooltip={{ text: absoluteTime(alert?.last_seen_at ?? ''), describes: true }}
						>
							{relativeTime(alert?.last_seen_at ?? '')}
						</time>
					{/snippet}
				</SettingRow>

				<SettingRow label={$t('alertPanel.rule')}>
					{#snippet chip()}
						{#if !alert?.rule}
							<span class="text muted" data-testid="alert-rule">{$t('alertPanel.deletedRule')}</span
							>
						{:else if isAdmin}
							<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- full path built by alertRulesHref -->
							<a class="link" data-testid="alert-rule" href={alertRulesHref(workspaceId, projectId)}
								>{clampText(alert.rule.name, NAME_MAX)}</a
							>
						{:else}
							<span class="text" data-testid="alert-rule"
								>{clampText(alert.rule.name, NAME_MAX)}</span
							>
						{/if}
					{/snippet}
				</SettingRow>

				<SettingRow label={$t('alertPanel.task')}>
					{#snippet chip()}
						<span class="task" data-testid="alert-task" data-state={task?.kind}>
							{#if task?.kind === 'created'}
								<StatusChip tone={task.tone}
									>{$t(task.labelKey, { values: task.values })}</StatusChip
								>
								<a class="link" href="#agent-tasks-title">{$t('alertPanel.viewTask')}</a>
							{:else if task?.kind === 'waiting'}
								<StatusChip tone="warning">{$t('alertPanel.taskWaiting')}</StatusChip>
								{#if task.until}
									<span class="text">
										{$t('alertPanel.taskWaitingUntil', {
											values: { time: absoluteTime(task.until) }
										})}
									</span>
								{/if}
							{:else if task?.kind === 'suppressed'}
								<StatusChip tone="warning">{$t('alertPanel.taskSuppressed')}</StatusChip>
							{:else if task?.kind === 'unknown'}
								<span class="mono">{task.code}</span>
							{:else}
								<span class="text muted">{$t('alertPanel.taskNone')}</span>
							{/if}
						</span>
					{/snippet}
				</SettingRow>
			</div>
		{/if}
	</section>
{/if}

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-6);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
		font-family: var(--rf-font-sans-stack);
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	.who {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		min-width: 0;
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-subheading);
		line-height: var(--rf-leading-subheading);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.rows {
		display: flex;
		flex-direction: column;
	}
	.text {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
		overflow-wrap: anywhere;
	}
	.muted {
		color: var(--rf-secondary);
	}
	.mono {
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
		overflow-wrap: anywhere;
	}
	.task {
		display: inline-flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
	.link {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
		text-decoration: underline;
		overflow-wrap: anywhere;
	}
</style>
