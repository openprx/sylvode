// Keep CI in sync with the registered contract suites as new Flow features land.
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const { scripts } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const suites = Object.keys(scripts).filter(
	(name) => name.startsWith('test:') && name !== 'test:ci'
);
if (suites.length === 0) throw new Error('No frontend test suites are registered');
const failures = [];
for (const suite of suites) {
	console.log(`::group::${suite}`);
	const result = spawnSync('bun', ['run', suite], { stdio: 'inherit' });
	console.log('::endgroup::');
	if (result.error || result.status !== 0) failures.push(suite);
}
console.log(`${suites.length} suites run; ${failures.length} failed`);
if (failures.length) {
	console.error(`Failed suites: ${failures.join(', ')}`);
	process.exit(1);
}
