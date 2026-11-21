// The wrapper every case runs in: records the outcome, keeps the evidence of a failure (secrets
// redacted) and rethrows with a redacted message so that Playwright prints nothing secret.

import type { Browser } from '@playwright/test';
import { redactor } from './redact.ts';
import { writeCaseResult, type CaseResult } from './record.ts';
import { errorMessage } from './util.ts';
import { CaseCtx, type World } from './world.ts';
import { CATALOG } from './catalog.ts';

export async function runCase(
	world: World,
	browser: Browser,
	id: string,
	body: (c: CaseCtx) => Promise<void>
): Promise<void> {
	const title = CATALOG.find((entry) => entry.id === id)?.title ?? id;
	const c = new CaseCtx(id, world, browser);
	const started = Date.now();
	let failure: string | undefined;
	try {
		await body(c);
	} catch (error) {
		failure = redactor.text(errorMessage(error));
	}
	await c.finish();
	if (failure === undefined && c.cleanupFailures.length > 0) {
		failure = `the case passed but its cleanup failed (${c.cleanupFailures.join(', ')}); the shared state may not be restored`;
	}
	const result: CaseResult = {
		id,
		title,
		status: failure === undefined ? 'passed' : 'failed',
		durationMs: Date.now() - started,
		timings: c.rec.timings,
		details: c.rec.details
	};
	if (failure !== undefined) {
		result.error = failure;
		result.evidence = { exchanges: c.rec.exchanges, notes: c.rec.notes };
	}
	// The result is the only thing written about the case: everything in it passed the redactor.
	writeCaseResult(world.cfg.outDir, redactor.json(result) as CaseResult);
	if (failure !== undefined) throw new Error(`${id} failed: ${failure}`);
}
