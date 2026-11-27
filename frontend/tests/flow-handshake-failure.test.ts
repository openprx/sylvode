/**
 * A collaboration handshake that never completes (`apps/api/src/flow/collab/session.rs`
 * `read_handshake_frame`; the plain-http outage where the server dropped a hello it could not
 * parse) must surface as its own failure, not as "no permission", "Flow is not enabled" or an
 * endless "Loading...", and must not be retried in a tight loop.
 *
 * Bounds under test (`object-session.ts`): a socket gets `HANDSHAKE_TIMEOUT_MS` (4 s) to produce
 * the server hello; after `HANDSHAKE_FAILURE_LIMIT` (3) consecutive sockets that end without one
 * the session stops and reports the Web-local `collab_handshake_failed`; between attempts it waits
 * at least `HANDSHAKE_RETRY_FLOOR_MS` (1 s) doubled per failure. The suite shortens the time
 * constants through the session options and checks the structure, not the wall clock.
 *
 * Run standalone: `bun tests/flow-handshake-failure.test.ts`
 */

import { Suite, assert, assertEqual, finish, waitFor } from './support/harness';
import {
	FakeCollabServer,
	FakeConnectivity,
	MockWebSocket,
	installFakes,
	tick
} from './support/collab-fakes';
import type { FlowError } from '../src/lib/flow/types';
import {
	HANDSHAKE_FAILURE_LIMIT,
	HANDSHAKE_RETRY_FLOOR_MS,
	HANDSHAKE_TIMEOUT_MS,
	LoroObjectSession
} from '../src/lib/flow/object-session';
import { flowErrorI18nKey } from '../src/lib/flow/errors';

const suite = new Suite('flow_handshake_failure');

const TIMEOUT_MS = 40;
const FLOOR_MS = 30;

suite.check('the documented bounds', () => {
	assertEqual(HANDSHAKE_TIMEOUT_MS, 4000, 'handshake bound');
	assertEqual(HANDSHAKE_FAILURE_LIMIT, 3, 'attempts before giving up');
	assertEqual(HANDSHAKE_RETRY_FLOOR_MS, 1000, 'retry floor');
});

async function harness(server: FakeCollabServer) {
	const restore = await installFakes(server);
	const errors: FlowError[] = [];
	const states: string[] = [];
	const socketTimes: number[] = [];
	const session = new LoroObjectSession(
		{
			onSnapshot: () => {},
			onAccepted: () => {},
			onFlowError: (error) => errors.push(error)
		},
		{
			createTicket: async () => {
				socketTimes.push(Date.now());
				return {
					code: 0,
					message: 'ok',
					data: { websocket_url: '/api/v2/flow/collab/ws?ticket=t', expires_at: '', ticket: 't' }
				} as never;
			}
		},
		{
			connectivity: new FakeConnectivity(),
			handshakeTimeoutMs: TIMEOUT_MS,
			handshakeRetryFloorMs: FLOOR_MS
		}
	);
	session.state.subscribe((state) => states.push(state));
	const handle = {
		objectId: '11111111-1111-4111-8111-111111111111',
		documentId: '22222222-2222-4222-8222-222222222222',
		scopeId: '33333333-3333-4333-8333-333333333333',
		workspaceId: '33333333-3333-4333-8333-333333333333',
		objectType: 'page',
		object: {} as never
	} as never;
	return {
		session,
		handle,
		errors,
		states,
		socketTimes,
		async done() {
			await session.dispose();
			restore();
		}
	};
}

async function rejection(promise: Promise<void>): Promise<FlowError | null> {
	try {
		await promise;
		return null;
	} catch (error) {
		return error as FlowError;
	}
}

for (const mode of ['silent', 'close'] as const) {
	await suite.checkAsync(
		`a server that ${mode === 'silent' ? 'never answers the hello' : 'drops the socket on the hello'} fails the connect as collab_handshake_failed`,
		async () => {
			const server = new FakeCollabServer();
			server.helloMode = mode;
			const ctx = await harness(server);
			try {
				const error = await rejection(ctx.session.connect(ctx.handle));
				assert(error !== null, 'connect rejected');
				assertEqual(
					error.code,
					'collab_handshake_failed',
					'the handshake failure, not another code'
				);
				assertEqual(error.origin, 'client', 'a Web-local code');
				assertEqual(error.recoverable, true, 'a retry is offered');
				assert(
					!['forbidden', 'feature_disabled', 'unauthenticated'].includes(error.code),
					'distinct from the permission, not-enabled and sign-in states'
				);
				assertEqual(
					flowErrorI18nKey(error),
					'flow.error.collab_handshake_failed',
					'its own message key'
				);
				assertEqual(
					MockWebSocket.instances.length,
					HANDSHAKE_FAILURE_LIMIT,
					'exactly the limit of sockets were tried'
				);
				assertEqual(server.hellos.length, HANDSHAKE_FAILURE_LIMIT, 'each socket said hello once');
				assertEqual(ctx.states.at(-1), 'error', 'the session is in the error state');
				assert(
					ctx.errors.some((entry) => entry.code === 'collab_handshake_failed'),
					'the flow error hook was told'
				);
				// Backoff: each retry waits at least the floor, doubling.
				const gaps = ctx.socketTimes.slice(1).map((time, i) => time - ctx.socketTimes[i]);
				assertEqual(gaps.length, HANDSHAKE_FAILURE_LIMIT - 1, 'two waits between three tries');
				assert(
					gaps[0] >= FLOOR_MS - 5,
					`first wait ${gaps[0]} ms honours the ${FLOOR_MS} ms floor`
				);
				assert(
					gaps[1] >= FLOOR_MS * 2 - 5,
					`second wait ${gaps[1]} ms honours the doubled ${FLOOR_MS * 2} ms floor`
				);
				// No tight loop afterwards: the session stays stopped.
				const opened = MockWebSocket.instances.length;
				await tick(TIMEOUT_MS * 6);
				assertEqual(MockWebSocket.instances.length, opened, 'no further sockets after giving up');
			} finally {
				await ctx.done();
			}
		}
	);
}

await suite.checkAsync(
	'one failed handshake followed by a good one connects normally',
	async () => {
		const server = new FakeCollabServer();
		server.helloMode = 'silent';
		const ctx = await harness(server);
		try {
			const connecting = ctx.session.connect(ctx.handle);
			await waitFor(() => MockWebSocket.instances.length >= 1, 'the first socket opened');
			server.helloMode = 'answer';
			const error = await rejection(connecting);
			assertEqual(error, null, 'connect resolved');
			assertEqual(MockWebSocket.instances.length, 2, 'one retry was enough');
			assertEqual(ctx.session.isConnected, true, 'the session is live');
			assertEqual(ctx.errors.length, 0, 'no error was raised');
		} finally {
			await ctx.done();
		}
	}
);

await suite.checkAsync('a server hello resets the count', async () => {
	const server = new FakeCollabServer();
	const ctx = await harness(server);
	try {
		assertEqual(await rejection(ctx.session.connect(ctx.handle)), null, 'first connect works');
		// Two silent handshakes after a good one are below the limit, so the session recovers.
		server.helloMode = 'silent';
		MockWebSocket.instances[MockWebSocket.instances.length - 1].serverClose(1006, '');
		await waitFor(() => MockWebSocket.instances.length >= 2, 'a reconnect started');
		await waitFor(() => MockWebSocket.instances.length >= 3, 'a second silent handshake was tried');
		server.helloMode = 'answer';
		await waitFor(() => ctx.session.isConnected, 'the session recovered');
		assertEqual(
			ctx.errors.some((entry) => entry.code === 'collab_handshake_failed'),
			false,
			'below the limit nothing is reported'
		);
	} finally {
		await ctx.done();
	}
});

export const result = suite.result();

if (import.meta.main) finish(result);
