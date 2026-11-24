import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import * as productMode from '../../src/lib/flow/product-mode';

// Collections are part of the Flow interface: nothing hides them behind a switch or a placeholder.

const ROOT = new URL('../..', import.meta.url).pathname;

function sources(dir: string, out: string[] = []): string[] {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) sources(path, out);
		else if (/\.(ts|svelte|json)$/.test(name)) out.push(path);
	}
	return out;
}

test('no switch, placeholder or page-only text is left in the source', () => {
	const offenders: string[] = [];
	for (const file of sources(join(ROOT, 'src'))) {
		const text = readFileSync(file, 'utf8');
		if (/FLOW_COLLECTION_UI_ENABLED|pageOnlyParent|flow\.pageOnly|"pageOnly"/.test(text))
			offenders.push(file);
	}
	assert.deepEqual(offenders, []);
});

test('the product mode module keeps only the reader compatibility constant', () => {
	assert.deepEqual(Object.keys(productMode), ['FLOW_PROCESS_READER_COMPATIBILITY']);
});

test('the navigator reads pages and collections, and the trash shows both', () => {
	const source = readFileSync(join(ROOT, 'src/lib/components/flow/FlowNavigator.svelte'), 'utf8');
	assert.match(source, /NAVIGATOR_OBJECT_TYPES = \['page', 'collection'\]/);
	assert.match(source, /item\.object_type === 'page' \|\| item\.object_type === 'collection'/);
});

test('the slash menu offers collections without a flag', () => {
	const source = readFileSync(join(ROOT, 'src/lib/components/flow/FlowCanvas.svelte'), 'utf8');
	assert.match(source, /option\.action !== 'convert' \|\| canConvert/);
});
