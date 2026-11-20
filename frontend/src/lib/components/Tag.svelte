<script lang="ts">
	import { t } from 'svelte-i18n';
	import X from '@lucide/svelte/icons/x';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';

	interface Props {
		label: string;
		/** Colour of the leading dot; a label colour chosen by a user, so it is data, not a token. */
		color?: string;
		removable?: boolean;
		onremove?: () => void;
	}

	let { label, color = 'var(--rf-secondary)', removable = false, onremove }: Props = $props();
</script>

<StatusChip tone="neutral">
	<span class="tag">
		<span class="dot" style:background-color={color}></span>
		<span>{label}</span>
		{#if removable}
			<button
				type="button"
				class="remove"
				onclick={onremove}
				aria-label={$t('activity.labelRemoved', { values: { name: label } })}
			>
				<X size={12} aria-hidden="true" />
			</button>
		{/if}
	</span>
</StatusChip>

<style>
	.tag {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
	}
	.dot {
		width: 8px;
		height: 8px;
		flex: none;
		border-radius: var(--rf-radius-full);
	}
	.remove {
		display: inline-grid;
		place-items: center;
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-xs);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
	}
	.remove:hover {
		background: var(--rf-hover);
		color: var(--rf-text);
	}
	.remove:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 1px;
	}
</style>
