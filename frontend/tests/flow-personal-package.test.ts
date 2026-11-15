import { test } from 'node:test';
import assert from 'node:assert/strict';
import { personalPackageReadiness } from '../src/lib/flow/personal-package';
test('personal package workflows fail closed independently of ordinary writes and missing flags', () => {
	for (const value of [
		null,
		{},
		{ api_version: 2, personal_writes: true },
		{ api_version: 1, personal_package_imports: true, personal_scope_package_exports: true }
	])
		assert.deepEqual(personalPackageReadiness(value), {
			imports: false,
			exports: false,
			historyImports: false,
			historyExports: false
		});
});
test('each actual workflow/history capability controls only its supported personal affordance', () => {
	assert.deepEqual(
		personalPackageReadiness({
			api_version: 2,
			personal_package_imports: true,
			personal_package_history_imports: true,
			personal_package_history_exports: true
		}),
		{ imports: true, exports: false, historyImports: true, historyExports: false }
	);
	assert.deepEqual(
		personalPackageReadiness({
			api_version: 2,
			personal_scope_package_exports: true,
			personal_package_export_selections: ['scope']
		}),
		{ imports: false, exports: true, historyImports: false, historyExports: false }
	);
});

test('object-only package export capability never enables a scope export card', () => {
	assert.equal(
		personalPackageReadiness({
			api_version: 2,
			personal_scope_package_exports: true,
			personal_package_export_selections: ['object']
		}).exports,
		false
	);
	assert.equal(
		personalPackageReadiness({ api_version: 2, personal_scope_package_exports: true }).exports,
		false
	);
});
