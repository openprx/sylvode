<script lang="ts">
	import type { HTMLTextareaAttributes } from 'svelte/elements';
	import { t } from 'svelte-i18n';
	import { uid } from '../foundations/ids';
	import { markingWord, type Marking } from '../form/field-state';

	interface Props extends Omit<HTMLTextareaAttributes, 'class' | 'children'> {
		label: string;
		value?: string;
		helper?: string;
		error?: string;
		/** This field's kind, for the marking word; same rule as `form/Field.svelte`. */
		requirement?: Marking;
		/** What the form marks (the minority kind); without it no word is shown. */
		marking?: Marking;
	}

	let {
		label,
		value = $bindable(''),
		helper,
		error,
		id = uid('textarea'),
		rows = 4,
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

<div class="ui-textarea">
	<label class="label" for={id}
		>{label}{#if word}<span class="word">&nbsp;{$t(`ui.form.${word}`)}</span>{/if}</label
	>
	<textarea
		{...rest}
		{id}
		{rows}
		bind:value
		{disabled}
		{readonly}
		required={required || requirement === 'required'}
		class:invalid={!!error}
		aria-invalid={error ? 'true' : undefined}
		aria-describedby={message ? messageId : undefined}
		data-ui="textarea"></textarea>
	{#if message}
		<span class="message" class:error={!!error} id={messageId}>{message}</span>
	{/if}
</div>

<style>
	.ui-textarea {
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
	textarea {
		padding: var(--rf-space-4) var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		resize: vertical;
	}
	textarea::placeholder {
		color: var(--rf-secondary);
	}
	textarea:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	textarea.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
	textarea:disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
		color: var(--rf-secondary);
		resize: none;
	}
	textarea:read-only:not(:disabled) {
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
</style>
