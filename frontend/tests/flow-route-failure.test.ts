import { routeFailure, routeFailureCopy } from '../src/lib/flow/route-failure';
import { Suite, assertEqual, finish } from './support/harness';
const suite = new Suite('Flow route failures');
suite.check('only explicit workspace disabled uses workspace toggle copy', () => {
	assertEqual(
		routeFailureCopy({
			workspaceId: 'workspace',
			...routeFailure({ code: 403, error_code: 'feature_disabled' })
		}).body,
		'flow.route.disabledBody',
		'explicit workspace flag'
	);
	for (const failure of [
		routeFailure({ code: 500 }),
		routeFailure({ code: 403 }),
		routeFailure({ code: 404 }),
		{}
	])
		assertEqual(
			routeFailureCopy({ workspaceId: 'workspace', ...failure }).body === 'flow.route.disabledBody',
			false,
			'other failures never claim flag disabled'
		);
});
suite.check('personal flags are distinct from workspace state', () => {
	assertEqual(
		routeFailureCopy(
			routeFailure({
				code: 403,
				error_code: 'feature_disabled',
				details: { reason: 'personal_creation_disabled' }
			})
		).body,
		'flow.route.personalBody',
		'personal capability'
	);
});
suite.check('auth notfound and network remain actionable without server-message branching', () => {
	for (const [code, key] of [
		[401, 'flow.errors.unauthenticated'],
		[403, 'flow.errors.forbidden'],
		[404, 'flow.route.notFoundBody'],
		[500, 'flow.route.networkBody']
	] as const)
		assertEqual(routeFailureCopy(routeFailure({ code })).body, key, 'typed/status failure');
});
finish(suite.result());
