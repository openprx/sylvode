// Evidence and results. Everything here is already redacted when it is stored.

import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export interface ExchangeRecord {
	label: string;
	method: string;
	url: string;
	requestBody: string | undefined;
	status: number;
	headers: Record<string, string>;
	body: string;
	elapsedMs: number;
}

/** Collects what a case did, so that a failure can show the raw exchanges (tokens redacted). */
export class Recorder {
	readonly exchanges: ExchangeRecord[] = [];
	readonly notes: string[] = [];
	readonly timings: Record<string, number> = {};
	readonly details: Record<string, unknown> = {};

	add(record: ExchangeRecord): void {
		this.exchanges.push(record);
		// Bounded: a long case must not produce an unbounded result file.
		if (this.exchanges.length > 200) this.exchanges.shift();
	}

	note(text: string): void {
		this.notes.push(text);
		if (this.notes.length > 300) this.notes.shift();
	}
}

export type CaseStatus = 'passed' | 'failed' | 'not_run';

export interface CaseResult {
	id: string;
	title: string;
	status: CaseStatus;
	durationMs: number;
	error?: string;
	timings: Record<string, number>;
	details: Record<string, unknown>;
	/** Present for failed cases only. */
	evidence?: { exchanges: ExchangeRecord[]; notes: string[] };
}

export function casesDir(outDir: string): string {
	return join(outDir, 'cases');
}

export function writeCaseResult(outDir: string, result: CaseResult): void {
	const dir = casesDir(outDir);
	mkdirSync(dir, { recursive: true });
	writeFileSync(join(dir, `${result.id}.json`), `${JSON.stringify(result, null, 2)}\n`, {
		mode: 0o600
	});
}
