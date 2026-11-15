// In-process developer fixture using the installed Loro library; no browser/API service.
import { LoroDoc } from 'loro-crdt';
import { readFileSync, writeFileSync } from 'node:fs';
import { keyBetween } from '../src/lib/flow/fractional-index';
const a = '11111111-1111-4111-8111-111111111111';
const b = '22222222-2222-4222-8222-222222222222';
const c = '33333333-3333-4333-8333-333333333333';
const directory = new URL('./fixtures/flow-navigator-order/', import.meta.url);
const assert = (condition: unknown, message: string) => {
	if (!condition) throw new Error(message);
};
const mode = process.argv[2];
if (mode === 'generate') {
	const doc = new LoroDoc();
	doc.setPeerId('4101');
	const order = doc.getMap('order');
	order.set(a, 'v');
	order.set(b, 'i');
	order.set(c, 'r');
	doc.getMap('meta').set('title', 'Navigator interoperability fixture');
	doc.getText('display-note').insert(0, 'Preserved display metadata');
	doc.commit();
	writeFileSync(new URL('browser.snapshot', directory), doc.export({ mode: 'snapshot' }));
	writeFileSync(
		new URL('manifest.json', directory),
		JSON.stringify({ a, b, c, map_id: order.id, initial_order: [b, c, a] }, null, 2) + '\n'
	);
	console.log('Generated map-only installed-library navigator fixture');
} else if (mode === 'consume') {
	const input = process.argv[3];
	if (!input) throw new Error('Rust snapshot path is required');
	const doc = new LoroDoc();
	doc.import(new Uint8Array(readFileSync(input)));
	const manifest = JSON.parse(readFileSync(new URL('manifest.json', directory), 'utf8'));
	const order = doc.getMap('order');
	assert(order.id === manifest.map_id, 'existing map identity changed');
	assert(order.get(b) === 'i' && order.get(c) === 'r', 'unmoved browser keys changed');
	assert(order.get(a) === 'm', 'server move did not use browser order-map bounds');
	assert(
		doc.getText('display-note').toString() === 'Preserved display metadata',
		'unrelated container changed'
	);
	assert(doc.getMap('meta').get('title') === 'Navigator interoperability fixture', 'title changed');
	const original = new LoroDoc();
	original.import(new Uint8Array(readFileSync(new URL('browser.snapshot', directory))));
	original.import(doc.export({ mode: 'update', from: original.version() }));
	assert(
		original.getMap('order').get(a) === 'm',
		'existing browser replica did not merge server move'
	);
	order.set(c, keyBetween(undefined, String(order.get(b))));
	doc.commit();
	writeFileSync(new URL('browser-reedit.snapshot', directory), doc.export({ mode: 'snapshot' }));
	console.log(
		'Passed map-only browser→Rust move→browser merge; generated subsequent browser reorder'
	);
} else throw new Error('Expected generate or consume mode');
