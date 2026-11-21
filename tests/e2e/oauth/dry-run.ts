// A mocked dry run of the whole suite against the in-process fake (tests/e2e/oauth/fake). It
// proves the client's own logic with no Sylvode at all: it starts the fake, runs the setup twice
// (the second time must change nothing), runs the real Playwright suite in headless Chromium
// against it, and scans the output for secrets on its own.
//
//   CHROMIUM_BIN=... OAUTH_E2E_OUT_DIR=<dir> node tests/e2e/oauth/dry-run.ts [--fault <name>] [--expect-fail A09,A16]
//
// Faults: none, p2-event, refresh-reuse, no-revoked-control. With --expect-fail the run succeeds
// only when exactly the named cases fail and every other case passes.

import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';
import { loadConfig, secretsOf } from './config.ts';
import { Fake, type Fault } from './fake/server.ts';
import { readCaseResults } from './lib/acceptance.ts';
import { TOKEN_PREFIXES } from './lib/redact.ts';
import { scanDirectory } from './lib/scan.ts';
import { runSetup } from './setup.ts';

const UI_PORT = Number(process.env.OAUTH_E2E_DRY_UI_PORT ?? 18456);
const MCP_PORT = Number(process.env.OAUTH_E2E_DRY_MCP_PORT ?? 18457);

function arg(name: string): string | undefined {
	const index = process.argv.indexOf(`--${name}`);
	return index === -1 ? undefined : process.argv[index + 1];
}

const fault = (arg('fault') ?? 'none') as Fault;
if (!['none', 'p2-event', 'refresh-reuse', 'no-revoked-control'].includes(fault)) {
	process.stderr.write(`unknown fault ${fault}\n`);
	process.exit(2);
}
const expectFail = (arg('expect-fail') ?? '').split(',').filter((id) => id !== '');
const outDir = process.env.OAUTH_E2E_OUT_DIR;
if (outDir === undefined || outDir === '') {
	process.stderr.write('OAUTH_E2E_OUT_DIR is required\n');
	process.exit(2);
}
rmSync(outDir, { recursive: true, force: true });

const password = (label: string): string => `Dry-${label}-${randomBytes(9).toString('hex')}`;
const env: Record<string, string> = {
	...(process.env as Record<string, string>),
	OAUTH_E2E_UI_ORIGIN: `http://127.0.0.1:${UI_PORT}`,
	OAUTH_E2E_API_ORIGIN: `http://127.0.0.1:${UI_PORT}`,
	OAUTH_E2E_MCP_ORIGIN: `http://127.0.0.1:${MCP_PORT}`,
	OAUTH_E2E_ADMIN_EMAIL: 'admin@dry.test',
	OAUTH_E2E_ADMIN_PASSWORD: password('admin'),
	OAUTH_E2E_USER_A_EMAIL: 'a@dry.test',
	OAUTH_E2E_USER_A_PASSWORD: password('a'),
	OAUTH_E2E_USER_B_EMAIL: 'b@dry.test',
	OAUTH_E2E_USER_B_PASSWORD: password('b'),
	OAUTH_E2E_OUT_DIR: outDir,
	OAUTH_E2E_FLOWS_PER_MINUTE: '1000',
	OAUTH_E2E_A20_OVERRIDE: resolve(import.meta.dirname, 'fake', 'a20-stub.sh'),
	// The fake server has no alert chain.
	OAUTH_E2E_SKIP_A21: '1',
	OAUTH_E2E_BUILD_COMMIT: 'dry-run-fake'
};
const cfg = loadConfig(env, 'run');
const fake = new Fake({ uiPort: UI_PORT, mcpPort: MCP_PORT, fault, admin: cfg.admin });
await fake.start();
process.stdout.write(`fake started (fault: ${fault}) on ${fake.ui} and ${fake.mcp}\n`);

let exitCode = 1;
try {
	process.stdout.write('--- setup, first run\n');
	await runSetup(cfg);
	process.stdout.write('--- setup, second run (must only find things)\n');
	await runSetup(cfg);

	const require = createRequire(import.meta.url);
	const cli = join(require.resolve('@playwright/test/package.json'), '..', 'cli.js');
	process.stdout.write('--- suite\n');
	const code = await new Promise<number>((done) => {
		const child = spawn(
			process.execPath,
			[cli, 'test', '-c', join(import.meta.dirname, 'playwright.config.ts')],
			{ env, stdio: 'inherit' }
		);
		child.on('exit', (status) => done(status ?? 1));
	});

	const results = readCaseResults(outDir);
	const failed = results
		.filter((r) => r.status === 'failed')
		.map((r) => r.id)
		.sort();
	const passed = results.filter((r) => r.status === 'passed').length;
	const scan = scanDirectory(outDir, [
		...TOKEN_PREFIXES.map((prefix) => ({ label: `prefix:${prefix.length}`, value: prefix })),
		...secretsOf(cfg)
	]);
	process.stdout.write(
		`--- result: ${passed} passed, failed: [${failed.join(', ')}]; independent secret scan: ${scan.findings.length} findings in ${scan.files} files\n`
	);
	if (scan.findings.length > 0) {
		process.stdout.write(
			`FINDINGS: ${scan.findings.map((f) => `${f.file} (${f.needle})`).join(', ')}\n`
		);
	}
	const wanted = [...expectFail].sort();
	const matches =
		failed.length === wanted.length &&
		failed.every((id, index) => id === wanted[index]) &&
		results.length === 20;
	if (scan.findings.length > 0) exitCode = 1;
	else if (expectFail.length === 0) exitCode = code === 0 && matches ? 0 : 1;
	else exitCode = matches && code !== 0 ? 0 : 1;
	process.stdout.write(
		`--- ${exitCode === 0 ? 'as expected' : 'NOT as expected'} (expected failures: [${wanted.join(', ')}], playwright exit ${code})\n`
	);
} finally {
	await fake.stop();
}
process.exit(exitCode);
