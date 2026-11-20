<script lang="ts" generics="V extends string = string">
	import type { Marking } from '../form/field-state';
	import SelectCore from '../select/SelectCore.svelte';
	import type { SelectOption } from '../select/listbox';

	interface Props {
		value: V[];
		options: SelectOption<V>[] | (() => Promise<SelectOption<V>[]>);
		onchange: (value: V[]) => void;
		label: string;
		labelHidden?: boolean;
		placeholder?: string;
		helper?: string;
		error?: string;
		required?: boolean;
		marking?: Marking;
		disabled?: boolean;
		disabledReason?: string;
		readonly?: boolean;
		searchable?: boolean | 'auto';
		density?: 'comfortable' | 'compact';
		/** Chips shown in the trigger before "+n"; default 2. */
		maxChips?: number;
		id?: string;
		testId?: string;
	}

	let { value, onchange, ...rest }: Props = $props();
</script>

<SelectCore {...rest} multiple values={value} onvalues={onchange} />
