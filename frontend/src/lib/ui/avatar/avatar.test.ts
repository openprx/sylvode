import { Suite, assert, assertEqual, finish } from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { initialsOf, shapeOf } from './avatar-state';

await installSvelteSsr();
const { default: Avatar } = await import('./Avatar.svelte');

const suite = new Suite('ui-avatar');

suite.check('initials take the first and last word, or the first character of one word', () => {
	assertEqual(initialsOf('Ada Lovelace'), 'AL', 'two words');
	assertEqual(initialsOf('lin'), 'L', 'one word, lower case');
	assertEqual(initialsOf('  Mary   Ann Smith  '), 'MS', 'extra spaces');
	assertEqual(initialsOf('王小明'), '王', 'CJK name');
	assertEqual(initialsOf(''), '', 'empty name');
});

suite.check('people and groups are circles, agents are rounded squares', () => {
	assertEqual(shapeOf('person'), 'circle', 'person');
	assertEqual(shapeOf('group'), 'circle', 'group');
	assertEqual(shapeOf('agent'), 'rounded-square', 'agent');
});

await suite.checkAsync('a named avatar is an image labelled by its name', async () => {
	const html = await renderToHtml(Avatar, { name: 'Ada Lovelace' });
	assert(html.includes('role="img"'), 'role img');
	assert(html.includes('aria-label="Ada Lovelace"'), 'accessible name');
	assert(html.includes('>AL<'), 'initials shown');
	assert(html.includes('shape-circle'), 'circle shape');
});

await suite.checkAsync('an agent avatar is a rounded square with its shape class', async () => {
	const html = await renderToHtml(Avatar, { name: 'Research Agent', kind: 'agent', size: 'sm' });
	assert(html.includes('shape-rounded-square'), 'rounded square');
	assert(html.includes('data-kind="agent"'), 'kind attribute');
	assert(html.includes('size-sm'), 'small size');
});

await suite.checkAsync('a decorative avatar is hidden from assistive technology', async () => {
	const html = await renderToHtml(Avatar, { name: 'Ada Lovelace', decorative: true });
	assert(html.includes('aria-hidden="true"'), 'aria-hidden');
	assert(!html.includes('role="img"'), 'no image role');
});

await suite.checkAsync('a group avatar shows the users glyph, not initials', async () => {
	const html = await renderToHtml(Avatar, { name: 'Design team', kind: 'group' });
	assert(html.includes('lucide-users') || html.includes('lucide lucide-users'), 'users glyph');
	assert(!html.includes('>DT<'), 'no initials');
});

finish(suite.result());
