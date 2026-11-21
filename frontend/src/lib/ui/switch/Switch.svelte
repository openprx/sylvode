<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';

	interface Props extends Omit<HTMLInputAttributes, 'class' | 'type' | 'children' | 'checked'> {
		checked?: boolean;
		label?: string;
	}

	let { checked = $bindable(false), label, disabled = false, ...rest }: Props = $props();
</script>

<label class="ui-switch" class:disabled>
	<span class="track-wrap">
		<input
			{...rest}
			bind:checked
			{disabled}
			type="checkbox"
			role="switch"
			data-ui="switch"
			data-filled={checked && !disabled ? '' : undefined}
		/>
		<span class="knob" aria-hidden="true"></span>
	</span>
	{#if label}<span class="text">{label}</span>{/if}
</label>

<style>
	.ui-switch {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-4);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		color: var(--rf-text);
		cursor: pointer;
	}
	.ui-switch.disabled {
		cursor: not-allowed;
	}
	.track-wrap {
		position: relative;
		display: inline-grid;
		flex: none;
		width: 36px;
		height: 20px;
	}
	input {
		grid-area: 1 / 1;
		appearance: none;
		margin: 0;
		width: 36px;
		height: 20px;
		border: 0;
		border-radius: var(--rf-radius-full);
		/* The off track uses the field boundary colour (D3). */
		background: var(--rf-input-border);
		cursor: inherit;
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	input:checked {
		background: var(--rf-primary);
	}
	input:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	input[data-filled]:focus-visible {
		outline-offset: 2px;
	}
	input:disabled {
		background: var(--rf-line-strong);
	}
	input:disabled:checked {
		background: var(--rf-control);
	}
	.knob {
		position: absolute;
		top: 2px;
		left: 2px;
		width: 16px;
		height: 16px;
		border-radius: var(--rf-radius-full);
		background: var(--rf-surface-white);
		pointer-events: none;
		transition: transform var(--rf-duration-fast) var(--rf-ease-standard);
	}
	input:checked + .knob {
		transform: translateX(16px);
		background: var(--rf-on-primary);
	}
	.ui-switch.disabled .text {
		color: var(--rf-secondary);
	}
	@media (pointer: coarse) {
		.ui-switch {
			min-height: var(--rf-touch-target);
		}
	}
</style>
