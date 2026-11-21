<svelte:options runes />
<script lang="ts">
	import { t } from 'svelte-i18n';
	import { tooltip } from '../../ui/tooltip/tooltip';
	import ClampedText from './ClampedText.svelte';
	import { eventChain, type ChainEntry, type ReplyLine } from './status';
	import { absoluteTime, relativeTime } from './time';
	import type { AiTaskEvent } from '../../api/ai-tasks';

	interface Props {
		events: AiTaskEvent[];
	}

	let { events }: Props = $props();

	const entries = $derived(eventChain(events));

	function label(entry: ChainEntry): string {
		switch (entry.kind) {
			case 'queued':
				return entry.position === null
					? $t('agentTasks.event.queued')
					: $t('agentTasks.event.queuedPosition', { values: { position: entry.position } });
			case 'progress':
				return entry.percent === null
					? $t('agentTasks.event.progress')
					: $t('agentTasks.event.progressPercent', { values: { percent: entry.percent } });
			case 'alive':
				return entry.count > 1
					? $t('agentTasks.event.aliveCount', { values: { count: entry.count } })
					: $t('agentTasks.event.alive');
			case 'other':
				return $t('agentTasks.event.other', { values: { type: entry.eventType } });
			default:
				return $t(`agentTasks.event.${entry.kind}`);
		}
	}

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

<ol class="chain" aria-label={$t('agentTasks.events')}>
	{#each entries as entry, index (index)}
		<li class="entry" data-kind={entry.kind}>
			<div class="head">
				<span class="label">{label(entry)}</span>
				<time
					class="when"
					datetime={entry.at}
					use:tooltip={{ text: absoluteTime(entry.at), describes: true }}
				>
					{relativeTime(entry.at)}
				</time>
			</div>
			{#if entry.detail}
				<ClampedText text={entry.detail} />
			{/if}
			{#if entry.reply.length > 0}
				<ul class="reply">
					{#each entry.reply as line, lineIndex (lineIndex)}
						<li>{replyText(line)}</li>
					{/each}
				</ul>
			{/if}
		</li>
	{/each}
</ol>

<style>
	.chain {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		border-inline-start: var(--rf-border-width) solid var(--rf-line);
		padding-inline-start: var(--rf-space-5);
	}
	.entry {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--rf-space-4);
	}
	.label {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		font-weight: var(--rf-weight-medium);
		color: var(--rf-text);
	}
	.when {
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
	.reply {
		margin: 0;
		padding-inline-start: var(--rf-space-6);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
		overflow-wrap: anywhere;
	}
</style>
