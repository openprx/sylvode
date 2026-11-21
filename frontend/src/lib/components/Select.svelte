<script module lang="ts">
	export interface SelectOption {
		label: string;
		value: string;
		disabled?: boolean;
	}

	/**
	 * The old select used the empty string for "nothing chosen"; the primitive uses null. An option
	 * whose value really is the empty string stays a normal choice.
	 */
	export function toPrimitiveValue(value: string, options: readonly SelectOption[]): string | null {
		return value === '' && !options.some((option) => option.value === '') ? null : value;
	}

	export function fromPrimitiveValue(next: string | null): string {
		return next ?? '';
	}
</script>

<script lang="ts">
	import UiSelect from '#lib/ui/select/Select.svelte';

	interface Props {
		label?: string;
		error?: string;
		hint?: string;
		helperText?: string;
		value?: string;
		options: SelectOption[];
		required?: boolean;
		disabled?: boolean;
		id?: string;
		placeholder?: string;
		/** Called after `value` has been updated; the new value is passed for convenience. */
		onchange?: (value: string) => void;
	}

	let {
		label,
		error,
		hint,
		helperText,
		value = $bindable(''),
		options,
		required = false,
		disabled = false,
		id,
		placeholder,
		onchange
	}: Props = $props();

	function choose(next: string | null) {
		value = fromPrimitiveValue(next);
		onchange?.(value);
	}
</script>

<UiSelect
	value={toPrimitiveValue(value, options)}
	{options}
	onchange={choose}
	label={label ?? placeholder ?? ''}
	labelHidden={!label}
	{placeholder}
	helper={helperText ?? hint}
	{error}
	{required}
	marking={required ? 'required' : undefined}
	{disabled}
	{id}
/>
