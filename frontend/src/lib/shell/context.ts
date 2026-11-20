import { getContext, setContext } from 'svelte';
import type { ShellState } from './shell-state.svelte';

export const SHELL_CONTEXT = 'sv-shell';

/** Called by the app layout during initialisation. */
export function provideShell(shell: ShellState): void {
	setContext(SHELL_CONTEXT, shell);
}

/** The shell state of the enclosing layout; null outside the app shell. */
export function getShell(): ShellState | null {
	try {
		return getContext<ShellState | undefined>(SHELL_CONTEXT) ?? null;
	} catch {
		return null;
	}
}
