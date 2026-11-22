<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { AiTask } from '../../api/ai-tasks';
	import Button from '../../ui/button/Button.svelte';
	import StatusChip from '../../ui/status-chip/StatusChip.svelte';
	import { tooltip } from '../../ui/tooltip/tooltip';
	import ClampedText from './ClampedText.svelte';
	import EventChain from './EventChain.svelte';
	import {
		attemptValues,
		botName,
		eventChain,
		latestProgress,
		statusView,
		taskReply,
		type ReplyLine
	} from './status';
	import { absoluteTime, relativeTime } from './time';

	interface Props {
		task: AiTask;
		/** Member names by user id, from the members list the page already loads. */
		names: Record<string, string>;
	}

	let { task, names }: Props = $props();

	let showEvents = $state(false);

	const status = $derived(statusView(task));
	const attempt = $derived(attemptValues(task));
	const progress = $derived(latestProgress(task));
	const reply = $derived(taskReply(task));
	const eventCount = $derived(eventChain(task.events).length);
	const active = $derived(task.status === 'processing' || task.status === 'acknowledged');
	const failureReason = $derived(
		task.status === 'failed' || task.status === 'retried' ? task.error_message : null
	);

	function replyText(line: ReplyLine): string {
		switch (line.kind) {
			case 'state':
				return $t('agentTasks.replyLine.state', { values: { from: line.from, to: line.to } });
			case 'vote':
				return $t('agentTasks.replyLine.vote', { values: { vote: line.vote } });
			case 'warning':
				return $t('agentTasks.replyLine.warning', { values: { text: line.text } });
			default:
				return $t(`agentTasks.replyLine.${line.kind}`);
		}
	}
</script>

<article class="task" data-testid="agent-task" data-status={task.status}>
	<header class="head">
		<div class="who">
			<span class="name"
				>{$t('agentTasks.agent', {
					values: { name: botName(names, task.ai_participant_id) }
				})}</span
			>
			<StatusChip tone={status.tone}>{$t(status.labelKey, { values: status.values })}</StatusChip>
			{#if attempt}
				<span class="meta">{$t('agentTasks.attempt', { values: attempt })}</span>
			{/if}
		</div>
		<time
			class="meta"
			datetime={task.created_at}
			use:tooltip={{ text: absoluteTime(task.created_at), describes: true }}
		>
			{relativeTime(task.created_at)}
		</time>
	</header>

	{#if task.plan}
		<section class="block">
			<h4 class="label">{$t('agentTasks.plan')}</h4>
			<ClampedText text={task.plan} />
		</section>
	{/if}

	{#if progress}
		<section class="block">
			<h4 class="label">
				{$t('agentTasks.progress')}
				{#if progress.percent !== null}
					<span class="meta"
						>{$t('agentTasks.progressPercent', { values: { percent: progress.percent } })}</span
					>
				{/if}
				{#if progress.at}
					<time
						class="meta"
						datetime={progress.at}
						use:tooltip={{ text: absoluteTime(progress.at), describes: true }}
					>
						{relativeTime(progress.at)}
					</time>
				{/if}
			</h4>
			{#if progress.note}
				<ClampedText text={progress.note} />
			{/if}
		</section>
	{:else if active && task.status === 'acknowledged'}
		<p class="meta">{$t('agentTasks.progressNone')}</p>
	{/if}

	{#if failureReason}
		<section class="block danger">
			<h4 class="label">{$t('agentTasks.reason')}</h4>
			<ClampedText text={failureReason} />
			{#if task.status === 'retried' && task.next_retry_at}
				<p class="meta">
					{$t('agentTasks.nextRetry', { values: { time: relativeTime(task.next_retry_at) } })}
				</p>
			{/if}
		</section>
	{/if}

	{#if reply}
		<section class="block">
			<h4 class="label">{$t('agentTasks.reply')}</h4>
			{#if reply.length === 0}
				<p class="meta">{$t('agentTasks.replyLine.none')}</p>
			{:else}
				<ul class="reply">
					{#each reply as line, index (index)}
						<li>{replyText(line)}</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}

	{#if eventCount > 0}
		<div class="events">
			<Button
				variant="text"
				size="sm"
				aria-expanded={showEvents}
				onclick={() => (showEvents = !showEvents)}
			>
				{showEvents
					? $t('agentTasks.hideEvents')
					: $t('agentTasks.showEvents', { values: { count: eventCount } })}
			</Button>
			{#if showEvents}
				<EventChain events={task.events} />
				{#if task.events_omitted}
					<p class="meta">{$t('agentTasks.eventsOmitted')}</p>
				{/if}
			{/if}
		</div>
	{/if}
</article>

<style>
	.task {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		padding: var(--rf-space-5) 0;
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		font-family: var(--rf-font-sans-stack);
		min-width: 0;
	}
	.task:last-child {
		border-bottom: none;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	.who {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
		min-width: 0;
	}
	.name {
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
		overflow-wrap: anywhere;
	}
	.meta {
		margin: 0;
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-regular);
		color: var(--rf-secondary);
	}
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.label {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: var(--rf-space-4);
		margin: 0;
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-secondary);
	}
	.danger .label {
		color: var(--rf-danger);
	}
	.reply {
		margin: 0;
		padding-inline-start: var(--rf-space-6);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
		overflow-wrap: anywhere;
	}
	.events {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--rf-space-4);
	}
</style>
