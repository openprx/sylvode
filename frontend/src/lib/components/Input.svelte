<script module lang="ts">
	/**
	 * The label the primitive prints: the caller's label, else the placeholder or name, which is then
	 * hidden visually so a field that had no visible label still has an accessible name.
	 */
	export function inputLabel(
		label: string | undefined,
		placeholder: string | undefined,
		name: string | undefined
	): { text: string; hidden: boolean } {
		if (label) return { text: label, hidden: false };
		return { text: placeholder ?? name ?? '', hidden: true };
	}
</script>

<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import '#lib/ui/foundations/visually-hidden.css';
	import TextField from '#lib/ui/text-field/TextField.svelte';

	type InputEventHandler = NonNullable<HTMLInputAttributes['oninput']>;

	interface Props {
		label?: string;
		error?: string;
		hint?: string;
		helperText?: string;
		type?: 'text' | 'email' | 'password' | 'number' | 'url' | 'tel' | 'search';
		placeholder?: string;
		value?: string | number;
		required?: boolean;
		disabled?: boolean;
		readonly?: boolean;
		maxlength?: number;
		minlength?: number;
		pattern?: string;
		id?: string;
		name?: string;
		autocomplete?: HTMLInputAttributes['autocomplete'];
		oninput?: InputEventHandler;
	}

	let {
		label,
		error,
		hint,
		helperText,
		type = 'text',
		placeholder,
		value = $bindable(''),
		required = false,
		disabled = false,
		readonly = false,
		maxlength,
		minlength,
		pattern,
		id,
		name,
		autocomplete,
		oninput
	}: Props = $props();

	const shown = $derived(inputLabel(label, placeholder, name));

	// A number input binds to a number at run time; the primitive types its value as a string.
	function getValue(): string {
		return value as string;
	}
	function setValue(next: string): void {
		value = next;
	}

	function hideLabel(node: HTMLElement, hidden: boolean) {
		const apply = (on: boolean) => node.querySelector('label')?.classList.toggle('ui-sr-only', on);
		apply(hidden);
		return { update: apply };
	}
</script>

<div class="ui-input-adapter" use:hideLabel={shown.hidden}>
	<TextField
		label={shown.text}
		bind:value={getValue, setValue}
		helper={helperText ?? hint}
		{error}
		requirement={required ? 'required' : undefined}
		marking={required ? 'required' : undefined}
		{type}
		{placeholder}
		{disabled}
		{readonly}
		{maxlength}
		{minlength}
		{pattern}
		{id}
		{name}
		{autocomplete}
		{oninput}
	/>
</div>
