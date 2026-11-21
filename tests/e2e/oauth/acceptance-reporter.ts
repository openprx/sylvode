// Playwright reporter: after the run, builds the acceptance record (markdown and JSON) from the
// per-case result files and runs the artifact self-check. Returning a failed status from
// onEnd turns a leaked artifact into a failed run.

import type { FullResult, Reporter } from '@playwright/test/reporter';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import {
	buildRecord,
	readCaseResults,
	renderMarkdown,
	type Environment
} from './lib/acceptance.ts';
import { loadConfig, secretsOf, type Config } from './config.ts';
import { redactor, TOKEN_PREFIXES } from './lib/redact.ts';
import { scanDirectory, type Needle } from './lib/scan.ts';
import { asRecord, dig, str } from './lib/util.ts';
import { call } from './lib/http.ts';
import { Recorder } from './lib/record.ts';
import { FirstParty, itemsOf } from './lib/sv.ts';

const PREFIX_LABELS: Record<string, string> = {
	sva_: 'token-prefix:access',
	svr_: 'token-prefix:refresh',
	svc_: 'token-prefix:code',
	sve_: 'token-prefix:embed',
	opr_: 'token-prefix:bot'
};

export function needlesFor(config: Config): Needle[] {
	return [
		...TOKEN_PREFIXES.map((prefix) => ({
			label: PREFIX_LABELS[prefix] ?? 'token-prefix',
			value: prefix
		})),
		...secretsOf(config)
	];
}

async function buildCommit(
	config: Config
): Promise<Pick<Environment, 'buildCommit' | 'buildCommitSource'>> {
	if (config.buildCommit !== undefined) {
		return { buildCommit: config.buildCommit, buildCommitSource: 'OAUTH_E2E_BUILD_COMMIT' };
	}
	try {
		const res = await call({
			rec: new Recorder(),
			label: 'health',
			method: 'GET',
			url: `${config.apiOrigin}/health`,
			timeoutMs: 10_000
		});
		const data = dig(res.json, 'data');
		const found =
			str(dig(data, 'git_commit')) ??
			str(dig(data, 'commit')) ??
			str(dig(data, 'build_commit')) ??
			str(dig(res.json, 'git_commit'));
		if (found !== undefined)
			return { buildCommit: found, buildCommitSource: `${config.apiOrigin}/health` };
	} catch {
		// fall through
	}
	return {
		buildCommit: 'unknown',
		buildCommitSource:
			'the API health endpoint exposes no build commit; set OAUTH_E2E_BUILD_COMMIT (api --build-info prints it)'
	};
}

async function clientActive(config: Config): Promise<boolean | null> {
	try {
		const admin = new FirstParty(config, 'admin', config.admin);
		const rec = new Recorder();
		const res = await admin.request(rec, 'list clients', 'GET', '/api/v1/admin/oauth-clients');
		const list = itemsOf(asRecord(res.json)?.data).map(asRecord);
		const status = str(list.find((c) => str(c?.client_id) === config.clientId)?.status);
		return status === undefined ? null : status === 'active';
	} catch {
		return null;
	}
}

export default class AcceptanceReporter implements Reporter {
	private startedAt = new Date().toISOString();

	onBegin(): void {
		this.startedAt = new Date().toISOString();
	}

	async onEnd(result: FullResult): Promise<{ status: FullResult['status'] } | undefined> {
		let config: Config;
		try {
			config = loadConfig(process.env, 'run');
		} catch (error) {
			process.stderr.write(
				`acceptance record not written: ${error instanceof Error ? error.message : String(error)}\n`
			);
			return { status: 'failed' };
		}
		mkdirSync(config.outDir, { recursive: true });
		const environment: Environment = {
			uiOrigin: config.uiOrigin,
			apiOrigin: config.apiOrigin,
			mcpOrigin: config.mcpOrigin,
			...(await buildCommit(config))
		};
		const results = readCaseResults(config.outDir);
		const active = await clientActive(config);
		const finishedAt = new Date().toISOString();

		// The record first, then the scan over everything in the output directory, including the
		// record itself; a finding is written to its own file under a label, never as a value.
		const emptyScan = { scannedFiles: 0, findings: [] };
		const draft = buildRecord(results, {
			startedAt: this.startedAt,
			finishedAt,
			environment,
			limitMs: config.limitMs,
			clientActive: active,
			scan: emptyScan
		});
		const template = readFileSync(join(import.meta.dirname, 'RESULTS-TEMPLATE.md'), 'utf8');
		const write = (record: typeof draft): void => {
			writeFileSync(
				join(config.outDir, 'acceptance.json'),
				`${JSON.stringify(redactor.json(record), null, 2)}\n`,
				{ mode: 0o600 }
			);
			writeFileSync(
				join(config.outDir, 'acceptance.md'),
				`${redactor.text(renderMarkdown(template, record))}\n`,
				{ mode: 0o600 }
			);
		};
		write(draft);
		const { files: scannedFiles, findings } = scanDirectory(config.outDir, needlesFor(config));
		const final = buildRecord(results, {
			startedAt: this.startedAt,
			finishedAt,
			environment,
			limitMs: config.limitMs,
			clientActive: active,
			scan: { scannedFiles, findings }
		});
		write(final);
		if (findings.length > 0) {
			// Rewriting the record with findings could itself contain the needle only if a file name did;
			// names are the paths of our own files.
			process.stderr.write(
				`ARTIFACT SELF-CHECK FAILED: ${findings.map((f) => `${f.file} (${f.needle})`).join(', ')}\n`
			);
		}
		const lines = final.cases.map((c) => `${c.id} ${c.status}`);
		process.stdout.write(
			`\nacceptance: ${final.result} (${final.summary.passed} passed, ${final.summary.failed} failed, ${final.summary.notRun} not run); record in ${config.outDir}\n${lines.join('\n')}\n`
		);
		if (final.result === 'failed' || findings.length > 0) return { status: 'failed' };
		return result.status === 'passed' ? undefined : { status: result.status };
	}
}
