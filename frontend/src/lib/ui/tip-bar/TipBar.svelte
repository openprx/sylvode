<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';
	import Info from '@lucide/svelte/icons/info';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';

	type Tone = 'neutral' | 'success' | 'warning' | 'danger';

	interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'class'> {
		/** Four tones; the glyph carries the state, the text stays in `text`. */
		tone?: Tone;
		/** One optional text action, such as "Retry". */
		action?: { label: string; run: () => void };
		children: Snippet;
	}

	let { tone = 'neutral', action, children, ...rest }: Props = $props();

	const glyphs = {
		neutral: Info,
		success: CircleCheck,
		warning: TriangleAlert,
		danger: CircleAlert
	};
	const Glyph = $derived(glyphs[tone]);
</script>

<div
	{...rest}
	class="ui-tip-bar tone-{tone}"
	role={tone === 'danger' ? 'alert' : 'status'}
	data-ui="tip-bar"
	data-tone={tone}
>
	<Glyph size={16} class="glyph" aria-hidden="true" />
	<span class="message">{@render children()}</span>
	{#if action}
		<button type="button" class="action" onclick={() => action.run()}>{action.label}</button>
	{/if}
</div>

<style>
	.ui-tip-bar {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		padding: var(--rf-space-4) var(--rf-space-5);
		border-radius: var(--rf-radius-card);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-text);
	}
	.ui-tip-bar :global(.glyph) {
		flex: none;
		color: var(--rf-secondary);
	}
	.message {
		flex: 1;
		min-width: 0;
	}
	.tone-neutral {
		background: var(--rf-surface);
	}
	.tone-success {
		background: var(--rf-success-bg);
	}
	.tone-success :global(.glyph) {
		color: var(--rf-success);
	}
	.tone-warning {
		background: var(--rf-warning-bg);
	}
	.tone-warning :global(.glyph) {
		color: var(--rf-warning);
	}
	.tone-danger {
		background: var(--rf-danger-bg);
	}
	.tone-danger :global(.glyph) {
		color: var(--rf-danger);
	}
	.action {
		flex: none;
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		font-weight: var(--rf-weight-semibold);
		cursor: pointer;
	}
	.action:hover {
		text-decoration: underline;
	}
	.action:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
</style>
