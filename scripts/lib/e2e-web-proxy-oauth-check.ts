// Checks that scripts/lib/e2e-web-proxy.ts handles the authorization server's paths the way
// frontend/nginx.conf does: the two discovery documents the API serves at the origin root are
// forwarded to the API (query and method intact), /.well-known/oauth-protected-resource/mcp goes
// to the MCP server only when E2E_PROXY_MCP_ORIGIN is set, /oauth/* answers the SPA with
// frame-ancestors 'none', X-Frame-Options: DENY, no-store and no referrer, and other pages do not
// get those headers. Needs Bun only: the API and the MCP server are stubs started here.
//
// Run: bun scripts/lib/e2e-web-proxy-oauth-check.ts
//   E2E_PROXY_CHECK_PORT   port of the proxy under test (default 18456)

import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const PORT = Number(process.env.E2E_PROXY_CHECK_PORT || 18456);
const seen: string[] = [];

const api = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(req) {
    const url = new URL(req.url);
    seen.push(`api ${req.method} ${url.pathname}${url.search}`);
    return Response.json(
      { from: "api", path: url.pathname },
      { headers: { "cache-control": "public, max-age=3600" } },
    );
  },
});
const mcp = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(req) {
    const url = new URL(req.url);
    seen.push(`mcp ${req.method} ${url.pathname}`);
    return Response.json({ from: "mcp", path: url.pathname });
  },
});

const dir = mkdtempSync(join(tmpdir(), "e2e-proxy-oauth-"));
writeFileSync(join(dir, "index.html"), "<!doctype html><title>spa</title>\n");

function start(port: number, withMcp: boolean) {
  return Bun.spawn(["bun", join(import.meta.dir, "e2e-web-proxy.ts")], {
    env: {
      ...process.env,
      E2E_PROXY_BUILD_DIR: dir,
      E2E_PROXY_API_ORIGIN: `http://127.0.0.1:${api.port}`,
      E2E_PROXY_PORT: String(port),
      E2E_PROXY_HOST: "127.0.0.1",
      E2E_PROXY_MCP_ORIGIN: withMcp ? `http://127.0.0.1:${mcp.port}` : "",
    },
    stdout: "ignore",
    stderr: "inherit",
  });
}

async function ready(port: number) {
  for (let i = 0; i < 100; i += 1) {
    try {
      await fetch(`http://127.0.0.1:${port}/health-probe`);
      return;
    } catch {
      await Bun.sleep(50);
    }
  }
  throw new Error(`proxy on ${port} did not start`);
}

let failures = 0;
const check = (ok: boolean, what: string) => {
  console.log(`${ok ? "PASS" : "FAIL"}: ${what}`);
  if (!ok) failures += 1;
};

const without = start(PORT, false);
const withMcp = start(PORT + 1, true);
try {
  await Promise.all([ready(PORT), ready(PORT + 1)]);
  const base = `http://127.0.0.1:${PORT}`;

  const as = await fetch(`${base}/.well-known/oauth-authorization-server?x=1`);
  const asBody = await as.json();
  check(
    as.status === 200 && asBody.from === "api",
    "authorization server metadata goes to the API",
  );
  check(
    seen.includes("api GET /.well-known/oauth-authorization-server?x=1"),
    "the query reaches the API unchanged",
  );
  check(
    as.headers.get("cache-control") === "public, max-age=3600",
    "the API's cache header is passed through",
  );

  const prm = await fetch(`${base}/.well-known/oauth-protected-resource/api`);
  check(
    (await prm.json()).from === "api",
    "protected resource metadata for the API goes to the API",
  );

  const mcpOff = await fetch(
    `${base}/.well-known/oauth-protected-resource/mcp`,
  );
  const mcpOffText = await mcpOff.text();
  check(
    mcpOffText.includes("<title>spa</title>") &&
      !seen.some((entry) => entry.startsWith("mcp ")),
    "without an MCP origin the MCP document is not proxied (SPA fallback, nothing sent upstream)",
  );
  check(
    !seen.some((entry) => entry.includes("oauth-protected-resource/mcp")),
    "the MCP document never reaches the API",
  );

  const mcpOn = await fetch(
    `http://127.0.0.1:${PORT + 1}/.well-known/oauth-protected-resource/mcp`,
  );
  check(
    (await mcpOn.json()).from === "mcp",
    "with an MCP origin the MCP document goes to the MCP server",
  );

  for (const path of [
    "/oauth/authorize",
    "/oauth/authorize?request=0b9c1b52-6a0e-4f6e-9a55-3a1f6a0d2c11",
  ]) {
    const page = await fetch(`${base}${path}`);
    const csp = page.headers.get("content-security-policy") ?? "";
    check(
      page.status === 200 && (await page.text()).includes("<title>spa</title>"),
      `${path} serves the SPA`,
    );
    check(
      csp.includes("frame-ancestors 'none'") &&
        !csp.includes("frame-ancestors 'self'"),
      `${path}: frame-ancestors 'none'`,
    );
    check(
      page.headers.get("x-frame-options") === "DENY",
      `${path}: X-Frame-Options DENY`,
    );
    check(
      page.headers.get("cache-control") === "no-store",
      `${path}: Cache-Control no-store`,
    );
    check(
      page.headers.get("referrer-policy") === "no-referrer",
      `${path}: Referrer-Policy no-referrer`,
    );
    check(
      page.headers.get("x-content-type-options") === "nosniff",
      `${path}: nosniff`,
    );
  }

  const other = await fetch(`${base}/workspace`);
  check(
    other.headers.get("x-frame-options") === null &&
      other.headers.get("cache-control") === null,
    "other pages do not get the /oauth/ headers",
  );

  const plainApi = await fetch(`${base}/api/v1/ping`);
  check((await plainApi.json()).from === "api", "/api/* is still forwarded");
} finally {
  without.kill();
  withMcp.kill();
  api.stop(true);
  mcp.stop(true);
}

if (failures > 0) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("e2e web proxy oauth routes: ok");
