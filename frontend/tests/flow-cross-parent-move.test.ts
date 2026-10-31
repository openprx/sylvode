/**
 * Cross-parent navigator moves build a `move_object` command and never offer a cycle.
 * Run standalone: `bun tests/flow-cross-parent-move.test.ts`
 */

import { Suite, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	descendantIds,
	moveInsideTargets,
	moveObjectCommand,
	moveOutsideTarget,
	type MoveNode
} from '../src/lib/flow/cross-parent-move';

const nodes: MoveNode[] = [
	{ id: 'root-page', parent_id: 'nav' },
	{ id: 'child', parent_id: 'root-page' },
	{ id: 'grandchild', parent_id: 'child' },
	{ id: 'sibling', parent_id: 'nav' }
];

const suite = new Suite('cross-parent move');

suite.check('move inside excludes self, descendants, and the current parent', () => {
	assertDeepEqual(moveInsideTargets(nodes, 'child'), ['sibling'], 'inside targets');
	assertEqual(descendantIds(nodes, 'child').has('grandchild'), true, 'grandchild is below child');
	assertEqual(descendantIds(nodes, 'child').has('sibling'), false, 'sibling is not a descendant');
});

suite.check('move outside uses the grandparent, including the navigator root', () => {
	assertEqual(moveOutsideTarget(nodes, 'child'), 'nav', 'child moves under the navigator');
	assertEqual(moveOutsideTarget(nodes, 'grandchild'), 'root-page', 'grandchild moves under the page');
	assertEqual(moveOutsideTarget(nodes, 'root-page'), null, 'a top-level page has no outside target');
});

suite.check('the command payload is move_object with the target parent', () => {
	assertDeepEqual(
		moveObjectCommand('nav'),
		{
			type: 'move_object',
			payload: { target_object_id: 'nav' }
		},
		'move_object payload'
	);
});

finish(suite.result());
