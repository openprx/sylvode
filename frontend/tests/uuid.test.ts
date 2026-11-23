/**
 * `randomUuid` (`src/lib/utils/uuid.ts`): the single id helper of the app. It must produce RFC 4122
 * version 4 UUIDs with and without `crypto.randomUUID` (a page served over plain http on a LAN
 * address has only `getRandomValues`), and must fail loudly rather than use `Math.random`.
 *
 * Run standalone: `bun tests/uuid.test.ts`
 */

import { Suite, assert, assertEqual, finish } from './support/harness';
import { randomUuid } from '../src/lib/utils/uuid';

const V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SAMPLES = 5000;

const suite = new Suite('uuid');

/** A crypto stand-in without `randomUUID`, as in an insecure context. */
function insecureCrypto(): Pick<Crypto, 'getRandomValues'> {
	return {
		getRandomValues: ((array: Uint8Array<ArrayBuffer>) =>
			globalThis.crypto.getRandomValues(array)) as never
	};
}

suite.check('default path returns v4 UUIDs', () => {
	for (let i = 0; i < SAMPLES; i += 1) assert(V4.test(randomUuid()), 'format');
});

suite.check('getRandomValues path: format, version and variant bits', () => {
	const source = insecureCrypto();
	const variants = new Set<string>();
	for (let i = 0; i < SAMPLES; i += 1) {
		const id = randomUuid(source);
		assert(V4.test(id), `format of ${id}`);
		assertEqual(id[14], '4', 'version nibble');
		variants.add(id[19]);
	}
	assertEqual([...variants].sort().join(''), '89ab', 'variant nibble takes all four values');
});

suite.check('bits are forced even for all-ones and all-zero input', () => {
	for (const fill of [0x00, 0xff]) {
		const source = {
			getRandomValues: ((array: Uint8Array) => array.fill(fill)) as never
		};
		const id = randomUuid(source);
		assert(V4.test(id), `format of ${id}`);
	}
	assertEqual(
		randomUuid({ getRandomValues: ((array: Uint8Array) => array.fill(0)) as never }),
		'00000000-0000-4000-8000-000000000000',
		'zero input'
	);
	assertEqual(
		randomUuid({ getRandomValues: ((array: Uint8Array) => array.fill(0xff)) as never }),
		'ffffffff-ffff-4fff-bfff-ffffffffffff',
		'all-ones input'
	);
});

suite.check('uniqueness over many samples on both paths', () => {
	const source = insecureCrypto();
	const seen = new Set<string>();
	for (let i = 0; i < SAMPLES; i += 1) {
		seen.add(randomUuid());
		seen.add(randomUuid(source));
	}
	assertEqual(seen.size, SAMPLES * 2, 'no duplicates');
});

suite.check('uses randomUUID when present', () => {
	let calls = 0;
	const id = randomUuid({
		randomUUID: () => {
			calls += 1;
			return '11111111-1111-4111-8111-111111111111';
		}
	});
	assertEqual(calls, 1, 'called once');
	assertEqual(id, '11111111-1111-4111-8111-111111111111', 'returned as is');
});

suite.check('the global path survives a stubbed crypto without randomUUID', () => {
	const original = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
	const stub = { getRandomValues: globalThis.crypto.getRandomValues.bind(globalThis.crypto) };
	Object.defineProperty(globalThis, 'crypto', { value: stub, configurable: true });
	try {
		assert(V4.test(randomUuid()), 'format without randomUUID');
	} finally {
		if (original) Object.defineProperty(globalThis, 'crypto', original);
	}
});

suite.check('throws a clear error when no secure random source exists, never Math.random', () => {
	const realRandom = Math.random;
	const original = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
	Math.random = () => {
		throw new Error('Math.random must not be used');
	};
	const messageOf = (call: () => unknown): string => {
		try {
			call();
		} catch (error) {
			return error instanceof Error ? error.message : String(error);
		}
		return '';
	};
	try {
		assert(messageOf(() => randomUuid({})).includes('getRandomValues'), 'empty source');
		Object.defineProperty(globalThis, 'crypto', { value: undefined, configurable: true });
		assert(messageOf(() => randomUuid()).includes('getRandomValues'), 'no global crypto');
	} finally {
		Math.random = realRandom;
		if (original) Object.defineProperty(globalThis, 'crypto', original);
	}
});

export const result = suite.result();

if (import.meta.main) finish(result);
