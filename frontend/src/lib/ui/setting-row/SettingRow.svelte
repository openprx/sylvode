<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAttributes } from 'svelte/elements';

	interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'class'> {
		label: string;
		/** Current value, in supporting text. */
		value?: string;
		/** Optional status chip next to the value. */
		chip?: Snippet;
		/** One secondary action, such as "Change". */
		action?: Snippet;
	}

	let { label, value, chip, action, ...rest }: Props = $props();
</script>

<div {...rest} class="ui-setting-row" data-ui="setting-row">
	<div class="main">
		<span class="label">{label}</span>
		{#if value || chip}
			<span class="meta">
				{#if value}<span class="value">{value}</span>{/if}
				{#if chip}{@render chip()}{/if}
			</span>
		{/if}
	</div>
	{#if action}<div class="action">{@render action()}</div>{/if}
</div>

<style>
	.ui-setting-row {
		display: flex;
		align-items: center;
		gap: var(--rf-space-5);
		padding: var(--rf-space-5) 0;
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		font-family: var(--rf-font-sans-stack);
	}
	.main {
		display: flex;
		flex: 1;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.label {
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-medium);
		color: var(--rf-text);
	}
	.meta {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
	}
	.value {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
		overflow-wrap: anywhere;
	}
	.action {
		flex: none;
	}
</style>
