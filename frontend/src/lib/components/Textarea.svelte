<script lang="ts">
	import type { HTMLTextareaAttributes } from 'svelte/elements';
	import '#lib/ui/foundations/visually-hidden.css';
	import UiTextarea from '#lib/ui/textarea/Textarea.svelte';
	import { inputLabel } from './Input.svelte';

	type TextareaEventHandler = NonNullable<HTMLTextareaAttributes['oninput']>;
	type TextareaPasteEventHandler = NonNullable<HTMLTextareaAttributes['onpaste']>;

	interface Props {
		label?: string;
		error?: string;
		hint?: string;
		helperText?: string;
		placeholder?: string;
		value?: string;
		rows?: number;
		required?: boolean;
		disabled?: boolean;
		readonly?: boolean;
		maxlength?: number;
		minlength?: number;
		id?: string;
		name?: string;
		oninput?: TextareaEventHandler;
		onpaste?: TextareaPasteEventHandler;
		element?: HTMLTextAreaElement | null;
	}

	let {
		label,
		error,
		hint,
		helperText,
		placeholder,
		value = $bindable(''),
		rows = 4,
		required = false,
		disabled = false,
		readonly = false,
		maxlength,
		minlength,
		id,
		name,
		oninput,
		onpaste,
		element = $bindable(null)
	}: Props = $props();

	const shown = $derived(inputLabel(label, placeholder, name));

	// The primitive does not expose its element, so the adapter finds it for `bind:element`.
	function track(node: HTMLElement, hidden: boolean) {
		const apply = (on: boolean) => node.querySelector('label')?.classList.toggle('ui-sr-only', on);
		element = node.querySelector('textarea');
		apply(hidden);
		return {
			update: apply,
			destroy() {
				element = null;
			}
		};
	}
</script>

<div class="ui-textarea-adapter" use:track={shown.hidden}>
	<UiTextarea
		label={shown.text}
		bind:value
		helper={helperText ?? hint}
		{error}
		requirement={required ? 'required' : undefined}
		marking={required ? 'required' : undefined}
		{placeholder}
		{rows}
		{disabled}
		{readonly}
		{maxlength}
		{minlength}
		{id}
		{name}
		{oninput}
		{onpaste}
	/>
</div>
