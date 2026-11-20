<script lang="ts">
	import { locale } from 'svelte-i18n';
	import type { HTMLAttributes } from 'svelte/elements';
	import { formatMetric } from './metric-state';

	interface Props extends Omit<HTMLAttributes<HTMLDivElement>, 'children' | 'class'> {
		/** Spoken first and shown above the figure. */
		label: string;
		value: number | string | null;
		/** Units or context below the figure, 12px. */
		note?: string;
		format?: Intl.NumberFormatOptions;
		loading?: boolean;
	}

	let { label, value, note, format, loading = false, ...rest }: Props = $props();

	const shown = $derived(formatMetric(value, $locale ?? undefined, format));
</script>

<div {...rest} class="ui-metric" aria-busy={loading || undefined} data-ui="metric">
	<span class="label">{label}</span>
	{#if loading}
		<span class="figure"><span class="block" aria-hidden="true"></span></span>
	{:else}
		<span class="figure">{shown}</span>
	{/if}
	{#if note}<span class="note">{note}</span>{/if}
</div>

<style>
	.ui-metric {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		font-family: var(--rf-font-sans-stack);
	}
	.label {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.figure {
		display: flex;
		align-items: center;
		min-height: calc(var(--rf-size-metric) * var(--rf-leading-metric));
		font-size: var(--rf-size-metric);
		line-height: var(--rf-leading-metric);
		font-weight: var(--rf-weight-semibold);
		font-variant-numeric: tabular-nums;
		color: var(--rf-text);
	}
	/* 96 by 28 block centred in the 40px line, so loading does not shift the card. */
	.block {
		display: block;
		width: 96px;
		height: 28px;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	.note {
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
</style>
