import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isFlowCanvasRoute } from '../src/lib/flow/page-layout';
test('personal and legacy canvas routes share a bounded viewport', () => {
	for (const route of [
		'/(app)/flow',
		'/(app)/flow/[objectId]',
		'/(app)/workspace/[workspaceId]/flow',
		'/(app)/workspace/[workspaceId]/flow/[objectId]'
	])
		assert.equal(isFlowCanvasRoute(route), true, route);
});
test('long forms and settings retain their ordinary page scroller', () => {
	for (const route of [
		null,
		'/(app)/flow/package',
		'/(app)/flow/[objectId]/convert',
		'/(app)/workspace/[workspaceId]/settings/flow',
		'/(app)/workspace/[workspaceId]/projects/[projectId]/forms'
	])
		assert.equal(isFlowCanvasRoute(route), false, String(route));
});
