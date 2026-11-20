<script lang="ts">
	import type { Snippet } from 'svelte';
	import { t } from 'svelte-i18n';
	import '../foundations/visually-hidden.css';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import { markingWord, type Marking } from './field-state';

	interface Props {
		/** The id of the control inside; the label points at it. */
		id: string;
		label: string;
		/** Hide the label visually (the control still gets its name from it). */
		labelHidden?: boolean;
		requirement?: Marking;
		/** What this form marks: the minority kind. Without it no word is shown. */
		marking?: Marking;
		/** Constraint or reason; replaced by the error line while there is an error. */
		helper?: string;
		error?: string | null;
		children: Snippet<[{ describedBy: string | undefined; invalid: boolean; required: boolean }]>;
	}

	let {
		id,
		label,
		labelHidden = false,
		requirement,
		marking,
		helper,
		error,
		children
	}: Props = $props();

	const messageId = $derived(`${id}-message`);
	const message = $derived(error || helper);
	const word = $derived(markingWord(requirement, marking));
</script>

<div class="ui-form-field">
	<label class="label" class:ui-sr-only={labelHidden} for={id} id="{id}-label"
		>{label}{#if word}<span class="word">&nbsp;{$t(`ui.form.${word}`)}</span>{/if}</label
	>
	{@render children({
		describedBy: message ? messageId : undefined,
		invalid: !!error,
		required: requirement === 'required'
	})}
	<!-- The container stays in the page so a blur error is announced when it appears. -->
	<span class="message" class:error={!!error} id={messageId} aria-live="polite">
		{#if message}
			{#if error}<CircleAlert size={14} aria-hidden="true" />{/if}
			<span class="text">{message}</span>
		{/if}
	</span>
</div>

<style>
	.ui-form-field {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		min-width: 0;
		font-family: var(--rf-font-sans-stack);
	}
	.label {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.message {
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-2);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		color: var(--rf-secondary);
	}
	.message:empty {
		display: none;
	}
	.message.error {
		color: var(--rf-danger);
	}
	.message :global(svg) {
		flex: none;
		margin-block-start: 1px;
	}
</style>
