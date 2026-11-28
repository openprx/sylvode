// Canary for scripts/run-tests.mjs, plain-script mode (`bun <file>`), the mode of the suites with
// their own harness. It rejects after an `await`; the runner must see a non-zero exit.
// Not matched by the `.test.ts` glob.
await new Promise((resolve) => setTimeout(resolve, 5));
throw new Error('deliberate canary failure');
