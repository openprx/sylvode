<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { absoluteTime, relativeTime } from '#lib/project/agent-tasks/time.js';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import RuleTestDialog from './RuleTestDialog.svelte';
	import SeverityChip from './SeverityChip.svelte';
	import { reportedJson, ruleNameOf } from './source-view';
	import { eventWorkItemHref, outcomeTone, outcomeWord } from './status';
	import type { InboundEvent, Rule } from './wire';

	interface Props {
		open: boolean;
		event: InboundEvent | null;
		/** The source the event belongs to; the rule test dialog starts on it. */
		sourceId: string;
		workspaceId: string;
		projectId: string;
		/** The project's rules, for the rule name; `null` while unknown. */
		rules: Rule[] | null;
		onclose: () => void;
	}
	let { open, event, sourceId, workspaceId, projectId, rules, onclose }: Props = $props();

	let testOpen = $state(false);
	// A different event starts from a closed test dialog.
	$effect(() => {
		if (!open || event === null) testOpen = false;
	});

	const word = $derived(event ? outcomeWord(event.outcome) : null);
	const href = $derived(event ? eventWorkItemHref(workspaceId, projectId, event) : null);
	const ruleName = $derived(event ? ruleNameOf(rules, event.rule_id) : null);
	const time = (iso: string): string => `${relativeTime(iso)} · ${absoluteTime(iso)}`;
</script>

<Dialog
	{open}
	size="wide"
	title={$t('inbound.events.detail.title', { values: { id: event?.event_id ?? '' } })}
	testId="event-detail-dialog"
	{onclose}
>
	{#if event}
		<div class="rows">
			<SettingRow label={$t('inbound.events.col.received')} value={time(event.received_at)} />
			<SettingRow label={$t('inbound.events.detail.occurred')} value={time(event.occurred_at)} />
			<SettingRow label={$t('inbound.events.col.kind')}>
				{#snippet chip()}<span class="mono">{event.kind}</span>{/snippet}
			</SettingRow>
			<SettingRow label={$t('inbound.events.col.severity')}>
				{#snippet chip()}<SeverityChip severity={event.severity_rank} />{/snippet}
			</SettingRow>
			<SettingRow label={$t('inbound.events.detail.dedupKey')}>
				{#snippet chip()}<span class="mono">{event.dedup_key}</span>{/snippet}
			</SettingRow>
			<SettingRow
				label={$t('inbound.events.col.signed')}
				value={event.signed ? $t('inbound.events.signedYes') : $t('inbound.events.signedNo')}
			/>
			<SettingRow label={$t('inbound.events.detail.from')} value={event.client_ip ?? '—'} />
			<SettingRow label={$t('inbound.events.col.outcome')}>
				{#snippet chip()}
					<StatusChip tone={outcomeTone(event.outcome)}>
						{#if word && word.key !== null}{$t(word.key)}{:else}<span class="mono"
								>{event.outcome}</span
							>{/if}
					</StatusChip>
				{/snippet}
			</SettingRow>
			<SettingRow label={$t('inbound.events.detail.rule')}>
				{#snippet chip()}
					{#if event.rule_id === null}
						—
					{:else if rules === null}
						<span class="mono">{event.rule_id}</span>
					{:else if ruleName !== null}
						<span class="value">{ruleName}</span>
					{:else}
						<span class="value">{$t('inbound.events.detail.deletedRule')}</span>
					{/if}
				{/snippet}
			</SettingRow>
			<SettingRow label={$t('inbound.events.col.workItem')}>
				{#snippet chip()}
					{#if href && event.work_item}
						<!-- eslint-disable svelte/no-navigation-without-resolve -- full path built by the caller -->
						<a {href} class="link">{event.work_item.identifier}</a>
						<!-- eslint-enable svelte/no-navigation-without-resolve -->
					{:else}
						—
					{/if}
				{/snippet}
			</SettingRow>
			<SettingRow label={$t('inbound.events.detail.task')}>
				{#snippet chip()}
					{#if event.task_id}<span class="mono">{event.task_id}</span>{:else}—{/if}
				{/snippet}
			</SettingRow>
		</div>

		<TipBar tone="neutral">{$t('inbound.events.untrusted')}</TipBar>

		<!-- The reported fields below were written by an external system. They are interpolated as
		     text only: never as markup, never as rich text, no links. -->
		<div class="blocks" data-testid="event-reported">
			<section class="block">
				<h4 class="caption">{$t('inbound.events.detail.reportedTitle')}</h4>
				<pre class="reported" data-testid="event-title">{event.title}</pre>
			</section>
			<section class="block">
				<h4 class="caption">{$t('inbound.events.detail.summary')}</h4>
				<pre class="reported" data-testid="event-summary">{event.summary}</pre>
			</section>
			<section class="block">
				<h4 class="caption">{$t('inbound.events.detail.subject')}</h4>
				<pre class="reported" data-testid="event-subject">{reportedJson(event.subject)}</pre>
			</section>
			<section class="block">
				<h4 class="caption">{$t('inbound.events.detail.payload')}</h4>
				<pre class="reported" data-testid="event-payload">{reportedJson(event.payload)}</pre>
			</section>
		</div>
	{/if}
	{#snippet actions()}
		<Button variant="secondary" disabled={event === null} onclick={() => (testOpen = true)}>
			{$t('inbound.events.detail.test')}
		</Button>
	{/snippet}
</Dialog>

{#if testOpen && event}
	<RuleTestDialog
		open
		onclose={() => (testOpen = false)}
		{projectId}
		{sourceId}
		eventId={event.id}
	/>
{/if}

<style>
	.rows {
		display: flex;
		flex-direction: column;
		margin-bottom: var(--rf-space-4);
	}
	.mono {
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
	}
	.value {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.link {
		color: inherit;
		text-decoration: underline;
	}
	.blocks {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		margin-top: var(--rf-space-4);
	}
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.caption {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		font-weight: 600;
	}
	.reported {
		margin: 0;
		padding: var(--rf-space-3);
		background: var(--rf-surface);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-input);
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
		max-height: 320px;
		overflow: auto;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
