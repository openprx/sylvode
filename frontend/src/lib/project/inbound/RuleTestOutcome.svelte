<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import RuleTestResult from './RuleTestResult.svelte';
	import type { TestState } from './test-view';

	interface Props {
		state: TestState;
	}
	let { state }: Props = $props();
</script>

<!-- The live region stays in the page so each new answer is announced. A state holds either a
     result or a failure, never both: a failed run cannot leave the last result on screen. -->
<div class="live" aria-live="polite" data-testid="rule-test-live" data-phase={state.phase}>
	{#if state.phase === 'running'}
		<p class="muted">{$t('inbound.test.running')}</p>
	{:else if state.phase === 'result'}
		<RuleTestResult result={state.result} />
	{:else if state.phase === 'error'}
		{#if state.failure.place === 'gone'}
			<TipBar tone="danger" data-testid="rule-test-error">{$t('inbound.test.eventGone')}</TipBar>
		{:else if state.failure.place === 'bar'}
			<TipBar tone="danger" data-testid="rule-test-error">
				{#if state.failure.forbidden}
					{$t('inbound.forbidden')}
				{:else}
					{$t('inbound.test.more.failed')} {state.failure.message}
				{/if}
			</TipBar>
		{/if}
	{/if}
</div>

<style>
	.muted {
		margin: 0;
		color: var(--rf-secondary);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
	}
</style>
