import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as pm from 'prosemirror-model';
import { createPageSchema } from '../src/lib/flow/editor-schema';
import { pageClipboardNode } from '../src/lib/flow/editor-clipboard';
import {
	issueBodyClipboard,
	trustedBodyClipboard,
	clearBodyClipboard
} from '../src/lib/flow/body/clipboard';
test('owned same-Page reference copies are accepted only with unchanged live origin, never external or cross-Page UUIDs', () => {
	const owner = {},
		other = {},
		blocks = [
			{
				type: 'image',
				attrs: {
					indent: 0,
					asset_id: '11111111-1111-4111-8111-111111111111',
					alt: 'Owned',
					caption: ''
				}
			}
		];
	const ticket = issueBodyClipboard(owner, blocks);
	assert(trustedBodyClipboard(owner, ticket, blocks));
	assert.equal(trustedBodyClipboard(other, ticket, blocks), false);
	assert.equal(
		trustedBodyClipboard(owner, ticket, [
			{ ...blocks[0], attrs: { ...blocks[0].attrs, asset_id: 'another' } }
		]),
		false
	);
	const schema = createPageSchema(pm);
	assert.throws(() => pageClipboardNode(blocks[0], schema));
	assert.equal(
		pageClipboardNode(blocks[0], schema, trustedBodyClipboard(owner, ticket, blocks)).type.name,
		'image'
	);
	clearBodyClipboard(owner);
	assert.equal(trustedBodyClipboard(owner, ticket, blocks), false);
});
