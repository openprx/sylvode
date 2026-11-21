<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';

	interface Props extends Omit<HTMLAttributes<HTMLSpanElement>, 'children' | 'class'> {
		/**
		 * Status is words first: the chip always says the state. Colour is added only for
		 * warning-level facts; success and danger follow the same soft-tint rule.
		 */
		tone?: 'neutral' | 'success' | 'warning' | 'danger';
		children: Snippet;
	}

	let { tone = 'neutral', children, ...rest }: Props = $props();
</script>

<span {...rest} class="ui-status-chip tone-{tone}" data-ui="status-chip" data-tone={tone}>
	{@render children()}
</span>

<style>
	.ui-status-chip {
		display: inline-flex;
		align-items: center;
		padding: var(--rf-space-2) var(--rf-space-4);
		border-radius: var(--rf-radius-chip);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-regular);
		white-space: nowrap;
	}
	.tone-neutral {
		background: var(--rf-surface);
		color: var(--rf-text);
	}
	.tone-success {
		background: var(--rf-success-bg);
		color: var(--rf-success);
	}
	.tone-warning {
		background: var(--rf-warning-bg);
		color: var(--rf-warning);
	}
	.tone-danger {
		background: var(--rf-danger-bg);
		color: var(--rf-danger);
	}
</style>
