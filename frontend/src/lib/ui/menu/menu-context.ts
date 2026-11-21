import { getContext, setContext } from 'svelte';

export interface MenuContext {
	close: () => void;
}

const KEY = Symbol('ui-menu');

export function provideMenu(context: MenuContext): void {
	setContext(KEY, context);
}

export function useMenu(): MenuContext | undefined {
	return getContext<MenuContext | undefined>(KEY);
}
