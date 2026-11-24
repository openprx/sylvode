<svelte:options runes />

<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import { plural } from '#lib/ui/foundations/plural.js';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { testAlertWord, testCauseWord, testTaskWord, type Word } from './status';
	import { limitRows, ruleRows, verdictView } from './test-view';
	import type { TestResult } from './wire';

	interface Props {
		result: TestResult;
	}
	let { result }: Props = $props();

	// A server code the UI does not know prints as the code itself. Everything below comes from
	// the server or from an external system and is interpolated as text only.
	const text = (word: Word): string => (word.key === null ? word.code : $t(word.key));

	const verdict = $derived(verdictView(result));
	const rows = $derived(ruleRows(result));
	const limits = $derived(result.caps ? limitRows(result.caps) : []);
	const alertWord = $derived(testAlertWord(result.alert));
	const taskWord = $derived(testTaskWord(result.task));
	const causeWord = $derived(
		result.queue_cause === null ? null : testCauseWord(result.queue_cause)
	);

	/** The task sentence; the queue cause fills the `{cause}` of the "would wait" sentence. */
	function taskSentence(word: Word, cause: Word | null): string {
		if (word.key === null) return word.code;
		return $t(word.key, { values: { cause: cause ? text(cause) : '' } });
	}

	const verdictLine = $derived(
		verdict.kind === 'noMatch'
			? $t('inbound.test.verdict.noMatch')
			: $t('inbound.test.verdict.match', {
					values: {
						position: verdict.position,
						name: verdict.draft
							? `${verdict.name} (${$t('inbound.test.reason.draft')})`
							: verdict.name,
						alert: text(verdict.alert),
						task: taskSentence(verdict.task, verdict.cause)
					}
				})
	);
</script>

<div class="result" data-testid="rule-test-result">
	<p class="verdict" data-testid="rule-test-verdict">{verdictLine}</p>

	<div class="rows">
		<SettingRow label={$t('inbound.test.row.alert')}>
			{#snippet chip()}
				{#if alertWord.key === null}<code>{alertWord.code}</code>{:else}{text(alertWord)}{/if}
			{/snippet}
		</SettingRow>
		<SettingRow label={$t('inbound.test.row.task')}>
			{#snippet chip()}
				{#if taskWord.key === null}<code>{taskWord.code}</code>{:else}{taskSentence(
						taskWord,
						causeWord
					)}{/if}
			{/snippet}
		</SettingRow>
		{#if result.would_escalate}
			<SettingRow label={$t('inbound.test.row.escalation')} />
		{/if}
		<SettingRow
			label={plural($t, 'inbound.test.row.recipients', result.notify_recipients, $locale)}
		/>
		<SettingRow label={$t('inbound.test.row.title')}>
			{#snippet chip()}<code data-testid="rule-test-title">{result.title}</code>{/snippet}
		</SettingRow>
	</div>

	{#if limits.length > 0}
		<section>
			<h3>{$t('inbound.test.row.limits')}</h3>
			<ul class="plain">
				{#each limits as row (row.key)}
					<li>
						{$t(`inbound.test.limit.${row.key}`, { values: { value: row.value, max: row.max } })}
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section>
		<h3>{$t('inbound.test.row.rules')}</h3>
		<ol class="plain checked" data-testid="rule-test-checked">
			{#each rows as row (row.id)}
				<li data-state={row.state}>
					<span class="position">{row.position}</span>
					<span class="name">
						{row.name}{#if row.draft}
							<span class="draft"> ({$t('inbound.test.reason.draft')})</span>{/if}
					</span>
					{#if row.state === 'matched'}
						<StatusChip tone="success">{$t('inbound.test.reason.matched')}</StatusChip>
					{:else if row.state === 'reason'}
						<span class="reason">
							{#if row.reason.key === null}<code>{row.reason.code}</code>{:else}{$t(
									row.reason.key
								)}{/if}
						</span>
					{:else}
						<span class="reason muted">{$t('inbound.test.reason.notChecked')}</span>
					{/if}
				</li>
			{/each}
		</ol>
	</section>

	<p class="forecast">{$t('inbound.test.forecast')}</p>
</div>

<style>
	.result {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		font-family: var(--rf-font-sans-stack);
	}
	.verdict {
		margin: 0;
		font-size: var(--rf-size-title);
		font-weight: 600;
		overflow-wrap: anywhere;
	}
	.rows {
		display: flex;
		flex-direction: column;
	}
	h3 {
		margin: 0 0 var(--rf-space-2);
		font-size: var(--rf-size-secondary);
		font-weight: 600;
		color: var(--rf-secondary);
	}
	.plain {
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--rf-size-secondary);
	}
	.checked li {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		padding: var(--rf-space-3) 0;
		border-bottom: var(--rf-border-width) solid var(--rf-line);
	}
	.position {
		min-width: 1.5em;
		color: var(--rf-secondary);
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.draft,
	.muted {
		color: var(--rf-secondary);
	}
	code {
		font-family: var(--rf-font-mono-stack);
		overflow-wrap: anywhere;
	}
	.forecast {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
