/** An in-memory `KeyValueStore` for storage-backed forms modules. */

import type { KeyValueStore } from '../../src/lib/forms/view-precedence';

export class MemoryStore implements KeyValueStore {
	readonly data = new Map<string, string>();
	failWrites = false;

	getItem(key: string): string | null {
		return this.data.get(key) ?? null;
	}

	setItem(key: string, value: string): void {
		if (this.failWrites) throw new Error('quota exceeded');
		this.data.set(key, value);
	}

	removeItem(key: string): void {
		this.data.delete(key);
	}
}
