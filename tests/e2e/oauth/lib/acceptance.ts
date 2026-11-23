// The acceptance record: built from the per-case result files, rendered through
// the template in ../results-template.ts, and written as markdown and JSON.

import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { CATALOG } from './catalog.ts';
import type { CaseResult } from './record.ts';
import { asRecord, str } from './util.ts';
import { casesDir } from './record.ts';

export interface Environment {
	uiOrigin: string;
	apiOrigin: string;
	mcpOrigin: string;
	buildCommit: string;
	buildCommitSource: string;
}

export interface TimingFigures {
	A07_notification_ms: number | null;
	A08_task_created_ms: number | null;
	A08_task_status_changed_ms: number | null;
	A11_revoked_ms: number | null;
}

export interface ScanSummary {
	scannedFiles: number;
	findings: Array<{ file: string; needle: string }>;
}

export interface AcceptanceRecord {
	schema: 'sylvode.p4.acceptance.v1';
	startedAt: string;
	finishedAt: string;
	environment: Environment;
	limitMs: number;
	result: 'passed' | 'failed';
	summary: { passed: number; failed: number; notRun: number };
	timings: TimingFigures;
	finalState: { clientActive: boolean | null };
	secretScan: ScanSummary;
	cases: Array<Omit<CaseResult, 'evidence'>>;
}

export function readCaseResults(outDir: string): CaseResult[] {
	const dir = casesDir(outDir);
	let files: string[] = [];
	try {
		files = readdirSync(dir).filter((name) => name.endsWith('.json'));
	} catch {
		files = [];
	}
	const results: CaseResult[] = [];
	for (const file of files) {
		const parsed = asRecord(JSON.parse(readFileSync(join(dir, file), 'utf8')) as unknown);
		if (parsed !== null && str(parsed.id) !== undefined)
			results.push(parsed as unknown as CaseResult);
	}
	return results;
}

function figure(results: CaseResult[], id: string, name: string): number | null {
	const value = results.find((r) => r.id === id)?.timings[name];
	return typeof value === 'number' ? value : null;
}

export function buildRecord(
	results: CaseResult[],
	meta: {
		startedAt: string;
		finishedAt: string;
		environment: Environment;
		limitMs: number;
		clientActive: boolean | null;
		scan: ScanSummary;
	}
): AcceptanceRecord {
	const cases: AcceptanceRecord['cases'] = CATALOG.map((entry) => {
		const found = results.find((r) => r.id === entry.id);
		if (found === undefined) {
			return {
				id: entry.id,
				title: entry.title,
				status: 'not_run' as const,
				durationMs: 0,
				timings: {},
				details: {}
			};
		}
		const { evidence: _evidence, ...rest } = found;
		return rest;
	});
	const summary = {
		passed: cases.filter((c) => c.status === 'passed').length,
		failed: cases.filter((c) => c.status === 'failed').length,
		notRun: cases.filter((c) => c.status === 'not_run').length
	};
	const clean =
		summary.failed === 0 &&
		summary.notRun === 0 &&
		meta.scan.findings.length === 0 &&
		meta.clientActive !== false;
	return {
		schema: 'sylvode.p4.acceptance.v1',
		startedAt: meta.startedAt,
		finishedAt: meta.finishedAt,
		environment: meta.environment,
		limitMs: meta.limitMs,
		result: clean ? 'passed' : 'failed',
		summary,
		timings: {
			A07_notification_ms: figure(results, 'A07', 'notification_ms'),
			A08_task_created_ms: figure(results, 'A08', 'task_created_ms'),
			A08_task_status_changed_ms: figure(results, 'A08', 'task_status_changed_ms'),
			A11_revoked_ms: figure(results, 'A11', 'revoked_ms')
		},
		finalState: { clientActive: meta.clientActive },
		secretScan: meta.scan,
		cases
	};
}

function cell(text: string): string {
	return text.replace(/\|/g, '\\|').replace(/\r?\n/g, ' ');
}

export function renderMarkdown(template: string, record: AcceptanceRecord): string {
	const limit = record.limitMs;
	const t = record.timings;
	const timingRow = (name: string, source: string, value: number | null): string =>
		`| ${name} | ${source} | ${value === null ? 'not measured' : `${value} ms`} | ${value === null ? 'FAIL' : value <= limit ? 'ok' : 'FAIL'} |`;
	const timingsTable = [
		'| Figure | Measured from | Value | Limit check |',
		'|---|---|---|---|',
		timingRow(
			'A07 notification.created',
			'end of the response that assigned the item',
			t.A07_notification_ms
		),
		timingRow(
			'A08 task.created',
			'end of the response that created the task',
			t.A08_task_created_ms
		),
		timingRow(
			'A08 task.status_changed (pending to processing)',
			'first poll that observed the change (else the creation response)',
			t.A08_task_status_changed_ms
		),
		timingRow(
			'A11 revoked control message',
			'DELETE answer as seen by the browser',
			t.A11_revoked_ms
		)
	].join('\n');
	const casesTable = [
		'| Case | Status | Duration | Title |',
		'|---|---|---|---|',
		...record.cases.map(
			(c) => `| ${c.id} | ${c.status} | ${(c.durationMs / 1000).toFixed(1)} s | ${cell(c.title)} |`
		)
	].join('\n');
	const failures = record.cases.filter((c) => c.status !== 'passed');
	const failureText =
		failures.length === 0
			? 'None.'
			: failures
					.map((c) => `- ${c.id} (${c.status}): ${cell(c.error ?? 'did not run')}`)
					.join('\n');
	const scan =
		record.secretScan.findings.length === 0
			? `Clean: ${record.secretScan.scannedFiles} files scanned, no token prefix and no test password found.`
			: `FAILED: ${record.secretScan.findings.map((f) => `${f.file} (${f.needle})`).join(', ')}`;
	const values: Record<string, string> = {
		result: record.result.toUpperCase(),
		started: record.startedAt,
		finished: record.finishedAt,
		ui: record.environment.uiOrigin,
		api: record.environment.apiOrigin,
		mcp: record.environment.mcpOrigin,
		commit: record.environment.buildCommit,
		commit_source: record.environment.buildCommitSource,
		summary: `${record.summary.passed} passed, ${record.summary.failed} failed, ${record.summary.notRun} not run`,
		client_state:
			record.finalState.clientActive === null
				? 'could not be read'
				: record.finalState.clientActive
					? 'active'
					: 'DISABLED',
		timings_table: timingsTable,
		cases_table: casesTable,
		failures: failureText,
		scan
	};
	return template.replace(/\{\{([a-z_]+)\}\}/g, (match, key: string) => values[key] ?? match);
}
