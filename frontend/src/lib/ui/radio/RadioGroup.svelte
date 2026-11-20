<script lang="ts">
	import type { Snippet } from 'svelte';
	import { uid } from '../foundations/ids';
	import { roving } from '../foundations/roving';
	import { provideRadioGroup } from './radio-context';

	interface Props {
		/** Accessible name of the group; or use `labelledby`. */
		label?: string;
		labelledby?: string;
		value?: string;
		name?: string;
		disabled?: boolean;
		orientation?: 'vertical' | 'horizontal';
		onchange?: (value: string) => void;
		children: Snippet;
	}

	let {
		label,
		labelledby,
		value = $bindable(),
		name = uid('radio'),
		disabled = false,
		orientation = 'vertical',
		onchange,
		children
	}: Props = $props();

	provideRadioGroup({
		get name() {
			return name;
		},
		value: () => value,
		select: (next) => {
			value = next;
			onchange?.(next);
		},
		disabled: () => disabled
	});
</script>

<!-- Arrow keys move focus and selection together, with one tab stop for the group. -->
<div
	role="radiogroup"
	aria-label={label}
	aria-labelledby={labelledby}
	aria-orientation={orientation}
	class="ui-radio-group {orientation}"
	use:roving={{
		selector: 'input[type="radio"]',
		orientation: 'both',
		onmove: (el) => el.click()
	}}
>
	{@render children()}
</div>

<style>
	.ui-radio-group {
		display: flex;
		gap: var(--rf-space-4);
	}
	.vertical {
		flex-direction: column;
	}
	.horizontal {
		flex-direction: row;
		flex-wrap: wrap;
		gap: var(--rf-space-6);
	}
</style>
