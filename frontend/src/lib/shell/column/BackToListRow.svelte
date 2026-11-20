<script lang="ts">
	import { t } from 'svelte-i18n';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';

	interface Props {
		/** The column's title (the destination name or "Settings"). */
		label: string;
		onclick: () => void;
	}
	let { label, onclick }: Props = $props();
</script>

<button
	type="button"
	class="back"
	data-shell="back-to-list"
	aria-label={$t('shell.column.backToList', { values: { name: label } })}
	{onclick}
>
	<ChevronLeft size={16} aria-hidden="true" />
	<span>{label}</span>
</button>

<style>
	/* 44 high row above the content in the narrow layout. */
	.back {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		width: 100%;
		min-height: var(--rf-density-row);
		padding: 0 var(--rf-space-6);
		border: 0;
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		background: transparent;
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		font-weight: var(--rf-weight-medium);
		text-align: start;
		cursor: pointer;
	}
	.back:hover {
		background: var(--rf-hover);
	}
	.back:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	@media (pointer: coarse) {
		.back {
			min-height: var(--rf-touch-target);
		}
	}
</style>
