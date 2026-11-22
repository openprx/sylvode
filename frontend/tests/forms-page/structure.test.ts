/**
 * Structure guard of the forms page: every component under the page and view directories starts
 * with `<svelte:options runes />` (a component without it compiles in legacy mode and can throw
 * on the first user edit, which a first-render check does not see), and no file of the page
 * grows past its line budget.
 *
 * Run standalone: `bun tests/forms-page/structure.test.ts`
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { Suite, assert, finish } from '../support/harness';

const root = new URL('../../', import.meta.url).pathname;
const route = 'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms';
const componentDirs = ['src/lib/components/forms/page', 'src/lib/components/forms/views'];
const budgetDirs = [route, 'src/lib/forms', ...componentDirs];
const HARD_LIMIT = 3000;
const COMPONENT_BUDGET = 800;
const ROUTE_BUDGET = 600;

function walk(dir: string, found: string[] = []): string[] {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) walk(path, found);
		else found.push(path);
	}
	return found;
}

const lineCount = (file: string): number => {
	const text = readFileSync(file, 'utf8');
	return text === '' ? 0 : text.split('\n').length - (text.endsWith('\n') ? 1 : 0);
};

const suite = new Suite('forms-page/structure');

const components = componentDirs.flatMap((dir) =>
	walk(join(root, dir)).filter((file) => file.endsWith('.svelte'))
);

suite.check('the component directories hold components', () => {
	assert(components.length >= 40, `expected at least 40 components, found ${components.length}`);
});

suite.check('every component starts with <svelte:options runes />', () => {
	const missing = components
		.filter(
			(file) => readFileSync(file, 'utf8').split('\n')[0]?.trim() !== '<svelte:options runes />'
		)
		.map((file) => relative(root, file));
	assert(missing.length === 0, `missing the runes option: ${missing.join(', ')}`);
});

suite.check(`no file of the page is above ${HARD_LIMIT} lines`, () => {
	const over = budgetDirs
		.flatMap((dir) => walk(join(root, dir)))
		.filter((file) => lineCount(file) > HARD_LIMIT)
		.map((file) => `${relative(root, file)} (${lineCount(file)})`);
	assert(over.length === 0, `above the hard limit: ${over.join(', ')}`);
});

suite.check(`the route is at most ${ROUTE_BUDGET} lines`, () => {
	const file = join(root, route, '+page.svelte');
	assert(lineCount(file) <= ROUTE_BUDGET, `${lineCount(file)} lines`);
});

suite.check(`no page component is above ${COMPONENT_BUDGET} lines`, () => {
	const over = components
		.filter((file) => lineCount(file) > COMPONENT_BUDGET)
		.map((file) => `${relative(root, file)} (${lineCount(file)})`);
	assert(over.length === 0, `above the component budget: ${over.join(', ')}`);
});

finish(suite.result());
