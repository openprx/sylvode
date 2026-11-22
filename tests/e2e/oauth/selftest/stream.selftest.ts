import assert from 'node:assert/strict';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { test } from 'node:test';
import { Recorder } from '../lib/record.ts';
import { asControl, asEvent, EventStream } from '../lib/stream.ts';
import { RollingLimiter, sleep } from '../lib/util.ts';

async function withServer(
	handler: (req: IncomingMessage, res: ServerResponse) => void,
	run: (url: string) => Promise<void>
): Promise<void> {
	const server = createServer(handler);
	await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
	try {
		await run(`http://127.0.0.1:${(server.address() as AddressInfo).port}/stream`);
	} finally {
		server.closeAllConnections();
		await new Promise<void>((resolve) => server.close(() => resolve()));
	}
}

test('messages split across chunks are stamped on arrival with a monotonic clock, in order', async () => {
	let seenHeader: string | undefined;
	await withServer(
		(req, res) => {
			seenHeader = req.headers['last-event-id'] as string | undefined;
			res.writeHead(200, { 'content-type': 'text/event-stream; charset=utf-8' });
			res.write('event: sv.control\ndata: {"type":"hel');
			setTimeout(
				() => res.write('lo","cursor":"c0"}\n\n: keep-alive\n\nid: 9\nevent: sv.event\n'),
				40
			);
			setTimeout(
				() =>
					res.write(
						'data: {"type":"notification.created","project":{"id":"p1"},"subject":{"x":1}}\n\n'
					),
				80
			);
			setTimeout(
				() => res.write('event: sv.control\ndata: {"type":"revoked","reason":"user_revoked"}\n\n'),
				120
			);
			setTimeout(() => res.end(), 160);
		},
		async (url) => {
			const rec = new Recorder();
			const opened = await EventStream.open({
				rec,
				label: 'test stream',
				url,
				token: 'tok-not-secret',
				lastEventId: 'cur-1',
				types: 'a,b'
			});
			assert.equal(opened.status, 200);
			const stream = opened.stream;
			assert.ok(stream !== null);
			const hello = await stream.waitForControl('hello', 2000);
			assert.ok(hello !== null);
			assert.equal(asControl(hello)?.body.cursor, 'c0');
			const event = await stream.waitForEvent((e) => e.type === 'notification.created', 2000);
			assert.ok(event !== null);
			assert.equal(asEvent(event)?.projectId, 'p1');
			assert.equal(asEvent(event)?.frameId, '9');
			const revoked = await stream.waitForControl('revoked', 2000);
			assert.ok(revoked !== null);
			assert.equal(asControl(revoked)?.reason, 'user_revoked');
			assert.ok(await stream.untilEnded(2000));
			const times = stream.items.map((item) => item.t);
			assert.deepEqual(
				[...times].sort((a, b) => a - b),
				times,
				'timestamps never go backwards'
			);
			assert.ok(
				hello.t < event.t && event.t < revoked.t,
				'the later message carries the later stamp'
			);
			assert.equal(stream.resumeId(), '9');
			assert.ok(stream.items.some((item) => item.item.kind === 'comment'));
			assert.equal(stream.ended, true);
			assert.equal(seenHeader, 'cur-1');
			assert.ok(rec.exchanges[0]?.url.includes('types=a%2Cb'));
			assert.ok(
				!JSON.stringify(rec.exchanges).includes('tok-not-secret'),
				'the bearer value is not recorded'
			);
		}
	);
});

test('a refusal comes back as a status with no stream, and waiting times out cleanly on a quiet stream', async () => {
	await withServer(
		(_req, res) => {
			res.writeHead(401, {
				'content-type': 'application/json',
				'www-authenticate': 'Bearer error="invalid_token"'
			});
			res.end('{"error":"invalid_token"}');
		},
		async (url) => {
			const rec = new Recorder();
			const opened = await EventStream.open({ rec, label: 'refused', url, token: 'x' });
			assert.equal(opened.status, 401);
			assert.equal(opened.stream, null);
			assert.equal(rec.exchanges[0]?.headers['www-authenticate'], 'Bearer error="invalid_token"');
		}
	);
	await withServer(
		(_req, res) => {
			res.writeHead(200, { 'content-type': 'text/event-stream' });
			res.write(': keep-alive\n\n');
		},
		async (url) => {
			const opened = await EventStream.open({
				rec: new Recorder(),
				label: 'quiet',
				url,
				token: 'x'
			});
			assert.ok(opened.stream !== null);
			const started = performance.now();
			assert.equal(await opened.stream.waitFor((t) => asEvent(t) !== null, 200), null);
			assert.ok(performance.now() - started >= 190);
			await opened.stream.close();
			assert.equal(opened.stream.endReason, 'closed by the client');
		}
	);
});

test('the rolling limiter lets a burst through and then waits', async () => {
	const limiter = new RollingLimiter(2, 300);
	const started = Date.now();
	await limiter.wait();
	await limiter.wait();
	assert.ok(Date.now() - started < 100);
	await limiter.wait();
	assert.ok(Date.now() - started >= 280);
	await sleep(0);
});
