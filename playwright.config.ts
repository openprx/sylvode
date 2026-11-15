import { defineConfig, devices } from "@playwright/test";

const chromiumExecutable = process.env.CHROMIUM_BIN || "/usr/bin/chromium";
if (/google[ -]?chrome/i.test(chromiumExecutable)) {
  throw new Error(
    "Tests require local open-source Chromium; set CHROMIUM_BIN to its verified executable.",
  );
}

export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 60_000,
  expect: {
    timeout: 10_000,
  },
  // Retries collect a trace of the failing attempt; CI still fails the run when any test was
  // flaky (`stats.flaky > 0` in the JSON report, `.github/workflows/ci.yml` Web E2E job).
  retries: process.env.CI ? 2 : 0,
  reporter: [
    ["html"],
    ["list"],
    ["json", { outputFile: "test-results/results.json" }],
  ],
  use: {
    baseURL: process.env.BASE_URL || "http://localhost:3000",
    headless: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    launchOptions: {
      executablePath: chromiumExecutable,
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
