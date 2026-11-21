// Playwright configuration of the OAuth acceptance suite. Separate from the repository's
// playwright.config.ts: its own project, its own output, no traces, videos, screenshots or
// reports that could carry a secret. Run it with
//   npx playwright test -c tests/e2e/oauth/playwright.config.ts
//
// The test file is named cases.e2e.ts on purpose: the root configuration only matches
// *.spec.ts and *.test.ts, so the web E2E job and a bare `npx playwright test` never pick it up.

import { defineConfig, devices } from '@playwright/test';
import { join } from 'node:path';

const chromiumExecutable = process.env.CHROMIUM_BIN || '/usr/bin/chromium';
if (/google[ -]?chrome/i.test(chromiumExecutable)) {
	throw new Error(
		'Tests require local open-source Chromium; set CHROMIUM_BIN to its verified executable.'
	);
}

const outDir = process.env.OAUTH_E2E_OUT_DIR ?? '';

export default defineConfig({
	testDir: import.meta.dirname,
	testMatch: 'cases.e2e.ts',
	fullyParallel: false,
	workers: 1,
	retries: 0,
	timeout: 240_000,
	expect: { timeout: 10_000 },
	// Playwright's own artifacts stay inside the output directory so the self-check covers them.
	outputDir:
		outDir === ''
			? join(import.meta.dirname, '.pw-artifacts')
			: join(outDir, 'playwright-artifacts'),
	reporter: [['list'], ['./acceptance-reporter.ts']],
	use: {
		headless: true,
		trace: 'off',
		video: 'off',
		screenshot: 'off',
		launchOptions: { executablePath: chromiumExecutable }
	},
	projects: [{ name: 'oauth-acceptance', use: { ...devices['Desktop Chrome'] } }]
});
