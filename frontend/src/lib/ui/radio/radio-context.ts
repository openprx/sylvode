import { getContext, setContext } from 'svelte';

export interface RadioContext {
	name: string;
	/** Current value; read through the getter so items stay reactive. */
	value: () => string | undefined;
	select: (value: string) => void;
	disabled: () => boolean;
}

const KEY = Symbol('ui-radio-group');

export function provideRadioGroup(context: RadioContext): void {
	setContext(KEY, context);
}

export function useRadioGroup(): RadioContext | undefined {
	return getContext<RadioContext | undefined>(KEY);
}
