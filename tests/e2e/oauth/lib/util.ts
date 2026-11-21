// Small shared helpers: type guards, timing, and the assertion helpers every case uses.
//
// Assertions throw CheckFailure with a message that the caller built from redacted data only. The
// suite never passes a response value to a Playwright `expect`, so a failing run cannot print one.

export function asRecord(value: unknown): Record<string, unknown> | null {
	return typeof value === 'object' && value !== null && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: null;
}

export function asArray(value: unknown): unknown[] | null {
	return Array.isArray(value) ? value : null;
}

export function str(value: unknown): string | undefined {
	return typeof value === 'string' ? value : undefined;
}

export function num(value: unknown): number | undefined {
	return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

/** Reads a nested field of untyped JSON: `dig(body, 'data', 'tokens', 'access')`. */
export function dig(value: unknown, ...path: string[]): unknown {
	let current: unknown = value;
	for (const key of path) {
		const record = asRecord(current);
		if (record === null) return undefined;
		current = record[key];
	}
	return current;
}

export function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

/** Monotonic clock in milliseconds; never goes backwards when the wall clock is adjusted. */
export function mono(): number {
	return performance.now();
}

export function errorMessage(error: unknown): string {
	if (error instanceof Error) return error.message;
	return String(error);
}

export class CheckFailure extends Error {
	override name = 'CheckFailure';
}

export function fail(message: string): never {
	throw new CheckFailure(message);
}

export function check(condition: unknown, message: string): asserts condition {
	if (!condition) throw new CheckFailure(message);
}

export function checkEqual<T extends string | number | boolean | null | undefined>(
	actual: T,
	expected: T,
	label: string
): void {
	if (actual !== expected) {
		throw new CheckFailure(
			`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`
		);
	}
}

export function sameSet(a: Iterable<string>, b: Iterable<string>): boolean {
	const left = new Set(a);
	const right = new Set(b);
	if (left.size !== right.size) return false;
	for (const item of left) if (!right.has(item)) return false;
	return true;
}

export function escapeRegExp(text: string): string {
	return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function randomHex(bytes: number): string {
	const data = new Uint8Array(bytes);
	crypto.getRandomValues(data);
	return Array.from(data, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** Allows at most `limit` events in any rolling window; `wait()` sleeps until one more fits. */
export class RollingLimiter {
	private readonly stamps: number[] = [];
	private readonly limit: number;
	private readonly windowMs: number;

	constructor(limit: number, windowMs: number) {
		this.limit = limit;
		this.windowMs = windowMs;
	}

	async wait(): Promise<void> {
		for (;;) {
			const now = Date.now();
			while (this.stamps.length > 0 && now - (this.stamps[0] ?? now) >= this.windowMs)
				this.stamps.shift();
			if (this.stamps.length < this.limit) {
				this.stamps.push(now);
				return;
			}
			const oldest = this.stamps[0] ?? now;
			await sleep(Math.max(50, this.windowMs - (now - oldest) + 25));
		}
	}
}
