<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Button from '../../ui/button/Button.svelte';
	import { CLAMP_CHARS, clamp } from './status';

	interface Props {
		/** Text written by an agent: shown as plain text only, never as markup and never linked. */
		text: string;
		max?: number;
	}

	let { text, max = CLAMP_CHARS }: Props = $props();

	let expanded = $state(false);
	const clamped = $derived(clamp(text, max));
</script>

<div class="clamped">
	<p class="text" data-testid="clamped-text">
		{expanded || !clamped.truncated ? text : `${clamped.text}…`}
	</p>
	{#if clamped.truncated}
		<Button
			variant="text"
			size="sm"
			aria-expanded={expanded}
			onclick={() => (expanded = !expanded)}
		>
			{expanded ? $t('agentTasks.showLess') : $t('agentTasks.showMore')}
		</Button>
	{/if}
</div>

<style>
	.clamped {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.text {
		margin: 0;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
	}
</style>
