/**
 * Resolved navigation per scope, cached for the columns. A scope whose endpoint answers 404, or
 * fails, has no entry in the cache and the column falls back to the built-in rows, so an older
 * server (or a failed request) never empties the navigation.
 */
import { navApi } from './api';
import type { NavScope, ResolvedNav } from './types';

export type NavStatus = 'idle' | 'loading' | 'ready' | 'fallback';

export interface NavState {
	status: NavStatus;
	menu: ResolvedNav | null;
}

const IDLE: NavState = { status: 'idle', menu: null };

export type MenuLoader = (
	scope: NavScope,
	id: string
) => Promise<{ code: number; data?: ResolvedNav | null }>;

export class NavStore {
	#states = $state.raw<Record<string, NavState>>({});
	#generation: Record<string, number> = {};

	constructor(private readonly loader: MenuLoader = (scope, id) => navApi.menu(scope, id)) {}

	#key(scope: NavScope, id: string): string {
		return `${scope}:${id}`;
	}

	state(scope: NavScope, id: string): NavState {
		return this.#states[this.#key(scope, id)] ?? IDLE;
	}

	/** A failed reload keeps the menu that is shown. */
	#failed(current: NavState): NavState {
		return current.status === 'ready' ? current : { status: 'fallback', menu: null };
	}

	/** Loads a scope's menu once; `force` reloads it and keeps the shown menu meanwhile. */
	async load(scope: NavScope, id: string, force = false): Promise<void> {
		const key = this.#key(scope, id);
		const current = this.#states[key] ?? IDLE;
		if (!force && current.status !== 'idle') return;
		const mine = (this.#generation[key] ?? 0) + 1;
		this.#generation[key] = mine;
		if (current.status === 'idle')
			this.#states = { ...this.#states, [key]: { status: 'loading', menu: null } };
		let next: NavState;
		try {
			const result = await this.loader(scope, id);
			next =
				result.code === 0 && result.data
					? { status: 'ready', menu: result.data }
					: this.#failed(current);
		} catch {
			next = this.#failed(current);
		}
		if (this.#generation[key] === mine) this.#states = { ...this.#states, [key]: next };
	}

	/** Reloads every scope that was loaded (after a configuration or preference change). */
	invalidate(): Promise<void[]> {
		return Promise.all(
			Object.keys(this.#states).map((key) => {
				const [scope, ...rest] = key.split(':');
				return this.load(scope as NavScope, rest.join(':'), true);
			})
		);
	}
}

export const navStore = new NavStore();
