<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLInputAttributes } from 'svelte/elements';
	import { t } from 'svelte-i18n';
	import { uid } from '../foundations/ids';
	import { markingWord, type Marking } from '../form/field-state';

	interface Props extends Omit<HTMLInputAttributes, 'class' | 'children'> {
		/** Visible label above the box; the label is never placed inside the field. */
		label: string;
		value?: string;
		/** Constraint or hint, shown before any error. */
		helper?: string;
		/** Error message; replaces the helper line and marks the field invalid. */
		error?: string;
		/** This field's kind, for the marking word; same rule as `form/Field.svelte`. */
		requirement?: Marking;
		/** What the form marks (the minority kind); without it no word is shown. */
		marking?: Marking;
		/** Trailing 16px glyph or control inside the box. */
		trailing?: Snippet;
	}

	let {
		label,
		value = $bindable(''),
		helper,
		error,
		trailing,
		id = uid('field'),
		disabled = false,
		readonly = false,
		required = false,
		requirement,
		marking,
		...rest
	}: Props = $props();

	const messageId = $derived(`${id}-message`);
	const message = $derived(error ?? helper);
	const word = $derived(markingWord(requirement, marking));
</script>

<div class="ui-field">
	<label class="label" for={id}
		>{label}{#if word}<span class="word">&nbsp;{$t(`ui.form.${word}`)}</span>{/if}</label
	>
	<div class="box" class:invalid={!!error} class:disabled class:readonly>
		<input
			{...rest}
			{id}
			bind:value
			{disabled}
			{readonly}
			required={required || requirement === 'required'}
			aria-invalid={error ? 'true' : undefined}
			aria-describedby={message ? messageId : undefined}
			data-ui="text-field"
		/>
		{#if trailing}<span class="trailing">{@render trailing()}</span>{/if}
	</div>
	{#if message}
		<span class="message" class:error={!!error} id={messageId}>{message}</span>
	{/if}
</div>

<style>
	.ui-field {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		font-family: var(--rf-font-sans-stack);
	}
	.label {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.box {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		height: var(--rf-field-height);
		padding: 0 var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
	}
	/* The ring replaces the border: drawn on the box's own edge. */
	.box:focus-within {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	input {
		flex: 1;
		min-width: 0;
		height: 100%;
		padding: 0;
		border: 0;
		outline: 0;
		background: transparent;
		color: inherit;
		font: inherit;
		font-size: var(--rf-size-body);
	}
	input::placeholder {
		color: var(--rf-secondary);
	}
	.trailing {
		display: inline-flex;
		color: var(--rf-secondary);
	}
	.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
	.disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
		color: var(--rf-secondary);
	}
	.readonly {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
	}
	.message {
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
	.message.error {
		color: var(--rf-danger);
	}
	@media (pointer: coarse) {
		.box {
			min-height: var(--rf-touch-target);
		}
	}
</style>
