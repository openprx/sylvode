<script lang="ts" generics="V extends string = string">
	import type { Marking } from '../form/field-state';
	import SelectCore from './SelectCore.svelte';
	import type { SelectOption } from './listbox';

	interface Props {
		value: V | null;
		options: SelectOption<V>[] | (() => Promise<SelectOption<V>[]>);
		onchange: (value: V | null) => void;
		label: string;
		/** Hide the label visually when the surrounding layout prints its own text. */
		labelHidden?: boolean;
		/** Names the thing: "Choose a status", never "Select...". */
		placeholder?: string;
		helper?: string;
		error?: string;
		required?: boolean;
		/** What this form marks: the minority kind. Without it no word is shown. */
		marking?: Marking;
		disabled?: boolean;
		disabledReason?: string;
		readonly?: boolean;
		/** `auto` shows a search field above eight options. */
		searchable?: boolean | 'auto';
		density?: 'comfortable' | 'compact';
		/** A native select on a coarse pointer under 640px; default true. */
		nativeOnTouch?: boolean;
		/** List "None" first; choosing it calls `onchange(null)`. */
		allowEmpty?: boolean;
		id?: string;
		testId?: string;
	}

	let { value, onchange, ...rest }: Props = $props();
</script>

<SelectCore
	{...rest}
	values={value === null ? [] : [value]}
	onvalues={(next) => onchange(next[0] ?? null)}
/>
