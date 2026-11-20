// A20: the existing regression scripts, run as they are run today.

import { execFile } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { redactor } from './redact.ts';
import type { Config } from '../config.ts';

export interface ScriptRun {
	name: string;
	command: string;
	exitCode: number | null;
	durationMs: number;
	/** The last part of stdout and stderr, redacted. */
	outputTail: string;
	mode: 'repository script' | 'override';
}

export function repoRootOf(cfg: Config): string {
	return cfg.repoRoot ?? resolve(import.meta.dirname, '..', '..', '..', '..');
}

interface Spec {
	kind: 'mcp' | 'ui';
	name: string;
	file: string;
	args: string[];
	cwd: string;
	env: Record<string, string>;
	/** The path that must exist for the script to be considered present. */
	script: string;
	timeoutMs: number;
}

export function specsFor(cfg: Config): Spec[] {
	const root = repoRootOf(cfg);
	return [
		{
			kind: 'mcp',
			name: 'bot-token MCP regression script (scripts/test-mcp.sh)',
			file: 'bash',
			args: [join(root, 'scripts', 'test-mcp.sh')],
			cwd: root,
			env: { MCP_URL: cfg.mcpOrigin, OPENPR_MCP_BOT_TOKEN: cfg.oprBotToken ?? '' },
			script: join(root, 'scripts', 'test-mcp.sh'),
			timeoutMs: 180_000
		},
		{
			kind: 'ui',
			name: 'JWT UI smoke test (frontend/scripts/ui-behaviour.mjs)',
			file: process.execPath,
			args: ['scripts/ui-behaviour.mjs', '--base-url', cfg.uiOrigin, '--api', cfg.apiOrigin],
			cwd: join(root, 'frontend'),
			env: {
				P3X_U1_EMAIL: cfg.admin.email,
				P3X_U1_PASSWORD: cfg.admin.password,
				...(process.env.CHROMIUM_BIN === undefined
					? {}
					: { CHROMIUM_BIN: process.env.CHROMIUM_BIN })
			},
			script: join(root, 'frontend', 'scripts', 'ui-behaviour.mjs'),
			timeoutMs: 300_000
		}
	];
}

export function runSpec(cfg: Config, spec: Spec): Promise<ScriptRun> {
	const started = Date.now();
	const override = cfg.a20Override;
	const [file, args] = override === undefined ? [spec.file, spec.args] : [override, [spec.kind]];
	return new Promise<ScriptRun>((resolveRun) => {
		execFile(
			file,
			args,
			{
				cwd: spec.cwd,
				env: { ...process.env, ...spec.env },
				timeout: spec.timeoutMs,
				maxBuffer: 8 * 1024 * 1024
			},
			(error, stdout, stderr) => {
				const raw: unknown = error === null ? 0 : (error as { code?: unknown }).code;
				const code = typeof raw === 'number' ? raw : null;
				const combined = redactor.text(`${stdout}\n${stderr}`);
				resolveRun({
					name: spec.name,
					command:
						override === undefined
							? `${spec.file} ${spec.args.map((a) => redactor.text(a)).join(' ')}`
							: `(override) ${args.join(' ')}`,
					exitCode: code,
					durationMs: Date.now() - started,
					outputTail: combined.slice(-3000),
					mode: override === undefined ? 'repository script' : 'override'
				});
			}
		);
	});
}

export function scriptExists(spec: Spec, cfg: Config): boolean {
	return cfg.a20Override !== undefined || existsSync(spec.script);
}
