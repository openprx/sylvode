import { test } from 'node:test';
import assert from 'node:assert/strict';
import { personalPackageReadiness } from '../src/lib/flow/personal-package';
test('personal package workflows fail closed independently of ordinary writes and missing flags', () => {
	for (const value of [null, {}, { personal_writes: true }])
		assert.deepEqual(personalPackageReadiness(value), { imports: false, exports: false });
});
test('each actual workflow capability controls only its supported personal affordance', () => {
	assert.deepEqual(personalPackageReadiness({ personal_package_imports: true }), {
		imports: true,
		exports: false
	});
	assert.deepEqual(personalPackageReadiness({ personal_scope_package_exports: true }), {
		imports: false,
		exports: true
	});
});
