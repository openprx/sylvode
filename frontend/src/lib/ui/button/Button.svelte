<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import { uid } from '../foundations/ids';
	import { isFilled, type ButtonVariant } from './button-state';

	interface Props extends Omit<HTMLButtonAttributes, 'children' | 'class'> {
		variant?: ButtonVariant;
		/** 24 for an inline action in a dense row, 32 default, 36 in a few forms. */
		size?: 'sm' | 'md' | 'lg';
		/** Visible reason shown with the disabled button (required by the specification). */
		disabledReason?: string;
		loading?: boolean;
		/** Progress verb shown instead of the label while loading ("Saving..."). */
		loadingLabel?: string;
		children: Snippet;
	}

	let {
		variant = 'secondary',
		size = 'md',
		disabled = false,
		disabledReason,
		loading = false,
		loadingLabel,
		type = 'button',
		onclick,
		children,
		...rest
	}: Props = $props();

	const reasonId = uid('button-reason');
	const showReason = $derived(disabled && !!disabledReason);

	function handleClick(event: MouseEvent & { currentTarget: EventTarget & HTMLButtonElement }) {
		if (loading) {
			event.preventDefault();
			return;
		}
		(onclick as ((e: typeof event) => void) | undefined)?.(event);
	}
</script>

<span class="ui-button-wrap" class:with-reason={showReason}>
	<button
		{...rest}
		{type}
		class="ui-button size-{size} v-{variant}"
		class:loading
		{disabled}
		aria-busy={loading || undefined}
		aria-describedby={showReason ? reasonId : rest['aria-describedby']}
		data-ui="button"
		data-filled={isFilled(variant) && !disabled ? '' : undefined}
		onclick={handleClick}
	>
		<span class="content" class:hidden={loading}>{@render children()}</span>
		{#if loading}
			<span class="busy">
				<LoaderCircle size={16} class="spin" aria-hidden="true" />
				{loadingLabel ?? $t('ui.button.loading')}
			</span>
		{/if}
	</button>
	{#if showReason}
		<span class="reason" id={reasonId}>{disabledReason}</span>
	{/if}
</span>

<style>
	.ui-button-wrap {
		display: inline-flex;
	}
	.ui-button-wrap.with-reason {
		flex-direction: column;
		align-items: flex-start;
		gap: var(--rf-space-2);
	}
	.ui-button {
		position: relative;
		display: inline-grid;
		place-items: center;
		min-height: var(--rf-control-height);
		padding: 0 var(--rf-space-5);
		border: var(--rf-border-width) solid transparent;
		border-radius: var(--rf-radius-button);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		font-weight: var(--rf-weight-medium);
		white-space: nowrap;
		cursor: pointer;
		transition:
			background-color var(--rf-duration-fast) var(--rf-ease-standard),
			color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.ui-button.size-sm {
		min-height: 24px;
		padding: 0 var(--rf-space-4);
	}
	.ui-button.size-lg {
		min-height: 36px;
	}
	/* Label and busy content share one grid cell, so loading never changes the width. */
	.content,
	.busy {
		grid-area: 1 / 1;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--rf-space-3);
	}
	.content.hidden {
		visibility: hidden;
	}
	.ui-button :global(.spin) {
		animation: ui-button-spin 900ms linear infinite;
	}
	@keyframes ui-button-spin {
		to {
			transform: rotate(360deg);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.ui-button :global(.spin) {
			animation-duration: 3s;
		}
	}

	.ui-button:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.ui-button[data-filled]:focus-visible {
		outline-offset: 2px;
	}

	.v-primary {
		background: var(--rf-primary);
		color: var(--rf-on-primary);
	}
	.v-primary:hover:not(:disabled) {
		background: color-mix(in oklab, var(--rf-primary), var(--rf-bg) 12%);
	}
	.v-primary:active:not(:disabled) {
		background: color-mix(in oklab, var(--rf-primary), var(--rf-bg) 22%);
	}
	.v-secondary {
		background: var(--rf-surface-white);
		border-color: var(--rf-text);
		color: var(--rf-text);
	}
	.v-secondary:hover:not(:disabled) {
		background: var(--rf-hover);
	}
	.v-secondary:active:not(:disabled) {
		background: var(--rf-pressed);
	}
	.v-text {
		background: transparent;
		color: var(--rf-text);
	}
	.v-text:hover:not(:disabled) {
		background: var(--rf-hover);
	}
	.v-text:active:not(:disabled) {
		background: var(--rf-pressed);
	}
	.v-danger {
		background: var(--rf-danger);
		color: var(--rf-on-danger);
	}
	.v-danger:hover:not(:disabled) {
		background: color-mix(in oklab, var(--rf-danger), var(--rf-bg) 12%);
	}
	.v-danger:active:not(:disabled) {
		background: color-mix(in oklab, var(--rf-danger), var(--rf-bg) 22%);
	}
	.v-danger-quiet {
		background: var(--rf-danger-bg);
		color: var(--rf-danger);
	}
	.v-danger-quiet:hover:not(:disabled) {
		background: color-mix(in oklab, var(--rf-danger-bg), var(--rf-danger) 10%);
	}
	.v-danger-quiet:active:not(:disabled) {
		background: color-mix(in oklab, var(--rf-danger-bg), var(--rf-danger) 18%);
	}

	/* Disabled is its own variant, never opacity. */
	.ui-button:disabled {
		background: var(--rf-surface);
		border-color: transparent;
		color: var(--rf-control);
		cursor: not-allowed;
	}
	.ui-button.loading {
		cursor: progress;
	}
	.reason {
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}

	@media (pointer: coarse) {
		.ui-button {
			min-height: var(--rf-touch-target);
		}
	}
</style>
