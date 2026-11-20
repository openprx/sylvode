import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** A scratch directory; OAUTH_E2E_SCRATCH names where (default: the system temp directory). */
export function scratch(prefix: string): string {
	return mkdtempSync(join(process.env.OAUTH_E2E_SCRATCH ?? tmpdir(), prefix));
}
