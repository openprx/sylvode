import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { publicBodyAtom } from '../src/lib/flow/body/presentation';
import type { FlowPublicBlock } from '../src/lib/api/flow';
async function fixture() {
	const value = JSON.parse(
		await readFile(new URL('./fixtures/page-v5/public-v5.json', import.meta.url), 'utf8')
	);
	return (Array.isArray(value) ? value : (value.blocks ?? value.data?.blocks)) as FlowPublicBlock[];
}
test('real backend model5 public DTO maps native names without treating attachment labels as headers or revealing a child target', async () => {
	const blocks = await fixture();
	assert(Array.isArray(blocks));
	const kinds = blocks.map((block) => publicBodyAtom(block)?.kind);
	assert(kinds.includes('image'));
	assert(kinds.includes('attachment'));
	assert(kinds.includes('childPage'));
	const child = blocks.find((block) => block.kind === 'childPage')!;
	assert.equal(child.child_page_id, undefined);
	assert.equal(child.text, '');
	assert.deepEqual(
		publicBodyAtom({ ...child, child_page_id: 'malicious-target', text: 'private title' }),
		{ kind: 'childPage' }
	);
	const attachment = blocks.find((block) => block.kind === 'attachment')!;
	assert.equal(attachment.mime, 'application/json');
	assert.equal('mime' in publicBodyAtom(attachment)!, false);
});
test('actual Svelte public-child rendering never resolves a private URL, fetches a target, or displays redacted text', async () => {
	const blocks = await fixture();
	const child = blocks.find((block) => block.kind === 'childPage')!;
	const { compile } = await import('svelte/compiler');
	const { render } = await import('svelte/server');
	const { register, init, locale } = await import('../src/lib/i18n/svelte-i18n');
	register('en', async () => ({
		flow: { body: { privatePageLink: 'Linked page · separate access required' } }
	}));
	init({ fallbackLocale: 'en', initialLocale: 'en' });
	await locale.set('en');
	const source = await readFile(
		new URL('../src/lib/components/flow/FlowBodyChild.svelte', import.meta.url),
		'utf8'
	);
	let code = compile(source, { generate: 'server', filename: 'FlowBodyChild.svelte' }).js.code;
	const denied =
		'data:text/javascript,' +
		encodeURIComponent(
			'export function resolve(){throw new Error("A redacted public child must not resolve a private route")}'
		);
	code = code.replace(
		/from (["'])([^"']+)\1/g,
		(_all, quote, path) =>
			'from ' +
			quote +
			(path === '$app/paths'
				? denied
				: path === 'svelte-i18n'
					? new URL('../src/lib/i18n/svelte-i18n.ts', import.meta.url).href
					: path === '#lib/api/flow.js'
						? new URL('../src/lib/api/flow.ts', import.meta.url).href
						: import.meta.resolve(path)) +
			quote
	);
	const component = (
		await import('data:text/javascript;base64,' + Buffer.from(code).toString('base64'))
	).default;
	const html = render(component, {
		props: { childId: child.child_page_id ?? '', publicView: true }
	}).body;
	assert.match(html, /Linked page/);
	assert.doesNotMatch(html, /\/flow\/|data-child-id|malicious-target|private title/);
});
