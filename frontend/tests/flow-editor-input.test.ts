/**
 * Markdown triggers, inline wraps, safe links, the block clipboard, and navigator paging.
 *
 * Run standalone: `bun tests/flow-editor-input.test.ts`
 */

import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	acceptEditorInput,
	decodeBlocks,
	encodeBlocks,
	completedBlockTrigger,
	insertIndexAfterRemoval,
	matchBlockShortcut,
	matchInlineWrap,
	safeHref
} from '../src/lib/flow/editor-input';
import { mergeObjectPage, shouldLoadMore } from '../src/lib/flow/navigator-paging';
import { semanticDiffLines } from '../src/lib/flow/history-diff';

const suite = new Suite('flow-editor-input');

suite.check('IME composition suspends markdown and mark shortcuts', () => {
	assertEqual(acceptEditorInput({ composing: true, isComposing: false }), false, 'view composing');
	assertEqual(acceptEditorInput({ composing: false, isComposing: true }), false, 'event composing');
	assertEqual(acceptEditorInput({ composing: false, isComposing: false }), true, 'committed text');
});

suite.check('block triggers match only at the start and keep the remainder', () => {
	assertDeepEqual(
		matchBlockShortcut('#', ' Hello', ''),
		{
			type: 'heading',
			level: 1,
			deleteBefore: 1,
			remainder: 'Hello'
		},
		'heading 1 keeps the remainder'
	);
	assertDeepEqual(
		matchBlockShortcut('', '## Title', ''),
		{
			type: 'heading',
			level: 2,
			deleteBefore: 0,
			remainder: 'Title'
		},
		'heading 2 from one insertion'
	);
	assertDeepEqual(
		matchBlockShortcut('###', ' ', 'later'),
		{
			type: 'heading',
			level: 3,
			deleteBefore: 3,
			remainder: ''
		},
		'heading 3'
	);
	assertDeepEqual(
		matchBlockShortcut('-', ' ', ''),
		{
			type: 'bulletItem',
			deleteBefore: 1,
			remainder: ''
		},
		'bullet'
	);
	assertDeepEqual(
		matchBlockShortcut('', '```', ''),
		{
			type: 'codeBlock',
			deleteBefore: 0,
			remainder: ''
		},
		'code fence'
	);
	assertEqual(matchBlockShortcut('already ', '# ', ''), null, 'a trigger after text is typed');
	assertEqual(matchBlockShortcut('# ', 'x', ''), null, 'a finished trigger is not matched again');
	assertEqual(matchBlockShortcut('``', '`', 'more'), null, 'a fence with trailing text stays text');
	assertEqual(completedBlockTrigger('# ')?.type, 'heading', 'completed heading trigger');
	assertEqual(completedBlockTrigger('# ')?.level, 1, 'completed heading level');
	assertEqual(completedBlockTrigger('```')?.type, 'codeBlock', 'completed code fence');
	assertEqual(completedBlockTrigger('# Hello'), null, 'text after the trigger is not a completion');
});

suite.check('inline delimiters become marks and ignore empty or partial pairs', () => {
	assertDeepEqual(
		matchInlineWrap('**bold*', '*'),
		{
			mark: 'bold',
			openStart: 0,
			openLength: 2,
			contentStart: 2,
			contentEnd: 6,
			deleteTail: 1
		},
		'bold'
	);
	assertDeepEqual(
		matchInlineWrap('*word', '*'),
		{
			mark: 'italic',
			openStart: 0,
			openLength: 1,
			contentStart: 1,
			contentEnd: 5,
			deleteTail: 0
		},
		'italic'
	);
	assertDeepEqual(
		matchInlineWrap('~~no~', '~'),
		{
			mark: 'strike',
			openStart: 0,
			openLength: 2,
			contentStart: 2,
			contentEnd: 4,
			deleteTail: 1
		},
		'strike'
	);
	assertDeepEqual(
		matchInlineWrap('`code', '`'),
		{
			mark: 'code',
			openStart: 0,
			openLength: 1,
			contentStart: 1,
			contentEnd: 5,
			deleteTail: 0
		},
		'code'
	);
	assertEqual(matchInlineWrap('**', '*'), null, 'empty bold');
	assertEqual(matchInlineWrap('*', '*'), null, 'empty italic');
	assertEqual(matchInlineWrap('**word', '*'), null, 'a single star does not close bold');
});

suite.check('links accept http(s), mailto, site paths, and fragments only', () => {
	assert(safeHref('https://example.com/a'), 'https');
	assert(safeHref('http://example.com'), 'http');
	assert(safeHref('mailto:a@example.com'), 'mailto');
	assert(safeHref('/workspace/1'), 'absolute path');
	assert(safeHref('#section'), 'fragment');
	assertEqual(safeHref('javascript:alert(1)'), false, 'javascript');
	assertEqual(safeHref('data:text/html,hi'), false, 'data');
	assertEqual(safeHref('//evil.example'), false, 'protocol relative');
	assertEqual(safeHref('https://example.com/a b'), false, 'space');
});

suite.check('block clipboard round-trips and rejects an unsafe link', () => {
	const raw = encodeBlocks([
		{
			type: 'paragraph',
			indent: 1,
			text: 'Hello',
			marks: [{ start: 0, end: 5, type: 'bold' }]
		}
	]);
	const decoded = decodeBlocks(raw);
	assert(decoded, 'decoded');
	assertEqual(decoded?.[0]?.text, 'Hello', 'text');
	assertEqual(decoded?.[0]?.marks[0]?.type, 'bold', 'mark');
	assertEqual(decodeBlocks('not json'), null, 'garbage');
	assertEqual(
		decodeBlocks(
			JSON.stringify({
				schema: 'sylvode.flow.blocks.v1',
				blocks: [
					{
						type: 'paragraph',
						indent: 0,
						text: 'x',
						marks: [{ start: 0, end: 1, type: 'link', href: 'javascript:alert(1)' }]
					}
				]
			})
		),
		null,
		'unsafe link'
	);
});

suite.check('moving a block range accounts for the hole it leaves', () => {
	assertEqual(insertIndexAfterRemoval(1, 3, 0), 0, 'move up');
	assertEqual(insertIndexAfterRemoval(0, 2, 4), 2, 'move down');
	assertEqual(insertIndexAfterRemoval(1, 3, 2), null, 'destination inside the range');
});

suite.check('navigator pages append by id and scroll loads near the bottom', () => {
	const merged = mergeObjectPage(
		[{ id: 'a' }, { id: 'b' }],
		[{ id: 'b' }, { id: 'c' }]
	);
	assertDeepEqual(
		merged.map((item) => item.id),
		['a', 'b', 'c'],
		'order'
	);
	assertEqual(shouldLoadMore(0, 100, 500), false, 'top of a long list');
	assertEqual(shouldLoadMore(360, 100, 500), true, 'near the bottom');
	assertEqual(shouldLoadMore(0, 200, 200), false, 'already fits');
});

suite.check('a span-only semantic change is not reported as a move', () => {
	const lines = semanticDiffLines({
		nodes: {
			added: {},
			removed: {},
			changed: {
				block: {
					before: {
						kind: 'block',
						text: 'Hello',
						properties: {},
						deleted: false,
						spans: []
					},
					after: {
						kind: 'block',
						text: 'Hello',
						properties: {},
						deleted: false,
						spans: [{ kind: 'bold', start: 0, end: 5 }]
					}
				}
			}
		}
	});
	assert(lines, 'shape');
	assertEqual(lines?.[0]?.op, 'changed', 'op');
	assertEqual(lines?.[0]?.properties[0]?.key, 'spans', 'spans property');
});

export const result = suite.result();

if (import.meta.main) finish(result);
