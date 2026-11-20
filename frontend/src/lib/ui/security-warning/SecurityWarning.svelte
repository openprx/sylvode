<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';

	interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'class'> {
		/** Warning for a risk the user should weigh; danger for an action that cannot be undone. */
		tone?: 'warning' | 'danger';
		title: string;
		children: Snippet;
	}

	let { tone = 'warning', title, children, ...rest }: Props = $props();
</script>

<div {...rest} class="ui-security-warning tone-{tone}" data-ui="security-warning" data-tone={tone}>
	{#if tone === 'danger'}
		<CircleAlert size={18} class="glyph" aria-hidden="true" />
	{:else}
		<TriangleAlert size={18} class="glyph" aria-hidden="true" />
	{/if}
	<div class="text">
		<p class="title">{title}</p>
		<div class="body">{@render children()}</div>
	</div>
</div>

<style>
	.ui-security-warning {
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-4) var(--rf-space-5);
		padding: var(--rf-space-5);
		border-radius: var(--rf-radius-card);
		font-family: var(--rf-font-sans-stack);
		color: var(--rf-text);
	}
	.ui-security-warning :global(.glyph) {
		flex: none;
	}
	.text {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		font-weight: var(--rf-weight-semibold);
	}
	/* Body 13 at 1.5 (specification 2.5). */
	.body {
		font-size: var(--rf-size-secondary);
		line-height: 1.5;
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
</style>
