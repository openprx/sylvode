import { readFileSync } from "node:fs";
import {
  expect,
  test,
  type APIRequestContext,
  type BrowserContext,
  type Page,
} from "@playwright/test";

/**
 * Two people on one Flow Page against a real API + PostgreSQL + worker
 * (`versions/v0.5-collaboration.md` "Web 交互"; vF rows `multi_user` / `offline_recovery`).
 *
 * Over REST an owner creates a workspace with Flow enabled (member baseline `edit`), adds member
 * B and creates a Page. Then, in two browser contexts:
 *
 * 1. A types; B sees A's text and A's remote caret (presence, rendered as decorations only).
 * 2. B goes offline (`context.setOffline(true)`), edits three times: the indicator shows
 *    `offline` with 3 pending; B comes back online and all three edits reach A.
 * 3. B goes offline again and edits twice; meanwhile the owner lowers B to `view` (explicit `view`
 *    grant + inheritance off). When B comes back, the replay is refused by the policy re-check: B's
 *    canvas turns read-only, both edits are in the recovery draft (downloadable JSON listing each
 *    refused intent and its reason), and A never receives them.
 * 4. A, still the owner, keeps editing after the grant change on the connection it already has:
 *    the server re-sources the moved authorization epoch in place and accepts, so A opens no new
 *    WebSocket (the client no longer reconnects to second-guess a `policy_rejected`); B -- now
 *    read-only -- still receives A's edits live.
 *
 * Needs BASE_URL serving the built frontend with `/api` (WebSocket included) proxied to the API on
 * the same origin (`scripts/e2e-web-stack.sh up`). Registration after the first account needs an
 * instance admin: ADMIN_EMAIL / ADMIN_PASSWORD when both are set, otherwise the run registers the
 * first account.
 */

const runId = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
const owner = {
  email: `mu-owner-${runId}@e2e.sylvode.test`,
  password: `MuOwner-${runId}-1!`,
};
const member = {
  email: `mu-member-${runId}@e2e.sylvode.test`,
  password: `MuMember-${runId}-1!`,
};
const OWNER_NAME = `Owner ${runId}`;
const MEMBER_NAME = `Member ${runId}`;

type Envelope<T> = {
  code: number;
  message: string;
  data: T;
  error_code?: string;
};
type LoginData = {
  tokens: { access_token: string; refresh_token: string };
  user: { id: string; name?: string };
};

async function unwrap<T>(
  response: Awaited<ReturnType<APIRequestContext["get"]>>,
): Promise<T> {
  expect(
    response.ok(),
    `${response.url()} answered HTTP ${response.status()}`,
  ).toBe(true);
  const body = (await response.json()) as Envelope<T>;
  expect(body.code, `${response.url()}: ${body.message}`).toBe(0);
  return body.data;
}

async function login(
  request: APIRequestContext,
  who: { email: string; password: string },
): Promise<LoginData> {
  return unwrap<LoginData>(
    await request.post("/api/v1/auth/login", { data: who }),
  );
}

async function instanceAdmin(request: APIRequestContext): Promise<LoginData> {
  if (process.env.ADMIN_EMAIL && process.env.ADMIN_PASSWORD) {
    return login(request, {
      email: process.env.ADMIN_EMAIL,
      password: process.env.ADMIN_PASSWORD,
    });
  }
  const admin = {
    email: `mu-admin-${runId}@e2e.sylvode.test`,
    password: `MuAdmin-${runId}-1!`,
  };
  await unwrap(
    await request.post("/api/v1/auth/register", {
      data: { ...admin, name: "Multi Admin" },
    }),
  );
  return login(request, admin);
}

async function register(
  request: APIRequestContext,
  admin: LoginData,
  who: { email: string; password: string },
  name: string,
): Promise<LoginData> {
  await unwrap(
    await request.post("/api/v1/auth/register", {
      headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
      data: { ...who, name },
    }),
  );
  return login(request, who);
}

async function signedInPage(
  context: BrowserContext,
  session: LoginData,
): Promise<Page> {
  await context.addInitScript(
    ({ accessToken, refreshToken, authUser }) => {
      localStorage.setItem("auth_token", accessToken);
      localStorage.setItem("refresh_token", refreshToken);
      localStorage.setItem("auth_user", JSON.stringify(authUser));
      localStorage.setItem("locale", "en");
    },
    {
      accessToken: session.tokens.access_token,
      refreshToken: session.tokens.refresh_token,
      authUser: session.user,
    },
  );
  return context.newPage();
}

async function typeAtEnd(page: Page, text: string): Promise<void> {
  const editor = page.locator(".flow-canvas .ProseMirror");
  await editor.click();
  await page.keyboard.press("Control+End");
  await page.keyboard.insertText(text);
}

test("multi-user: presence, offline outbox replay, and a view downgrade into the recovery draft", async ({
  browser,
  request,
}) => {
  test.setTimeout(150_000);
  const admin = await instanceAdmin(request);
  const ownerLogin = await register(request, admin, owner, OWNER_NAME);
  const memberLogin = await register(request, admin, member, MEMBER_NAME);
  const auth = { Authorization: `Bearer ${ownerLogin.tokens.access_token}` };

  const workspace = await unwrap<{ id: string }>(
    await request.post("/api/v1/workspaces", {
      headers: auth,
      data: { slug: `mu-${runId}`, name: `Multi ${runId}` },
    }),
  );
  await unwrap(
    await request.post(`/api/v1/workspaces/${workspace.id}/members`, {
      headers: auth,
      data: { user_id: memberLogin.user.id, role: "member" },
    }),
  );
  await unwrap(
    await request.put(`/api/v1/workspaces/${workspace.id}/features/flow`, {
      headers: auth,
      data: {
        enabled: true,
        default_member_level: "edit",
        idempotency_key: crypto.randomUUID(),
      },
    }),
  );
  const created = await unwrap<{ object: { id: string } }>(
    await request.post(`/api/v1/workspaces/${workspace.id}/flow/objects`, {
      headers: auth,
      data: {
        object_type: "page",
        title: `Shared ${runId}`,
        idempotency_key: crypto.randomUUID(),
      },
    }),
  );
  const objectId = created.object.id;
  const url = `/workspace/${workspace.id}/flow/${objectId}`;

  const contextA = await browser.newContext();
  const contextB = await browser.newContext({ acceptDownloads: true });
  try {
    const pageA = await signedInPage(contextA, ownerLogin);
    const pageB = await signedInPage(contextB, memberLogin);
    let socketsA = 0;
    pageA.on("websocket", () => {
      socketsA += 1;
    });
    await pageA.goto(url);
    await pageB.goto(url);
    const editorA = pageA.locator(".flow-canvas .ProseMirror");
    const editorB = pageB.locator(".flow-canvas .ProseMirror");
    const indicatorB = pageB.getByTestId("flow-sync-indicator");
    await expect(editorA).toBeVisible();
    await expect(editorB).toBeVisible();

    // 1. A types; B sees the text and A's caret.
    await typeAtEnd(pageA, "Hello from A");
    await expect(editorB).toContainText("Hello from A");
    const caret = pageB.getByTestId("flow-presence-caret");
    await expect(caret).toBeVisible();
    await expect(caret).toHaveAttribute(
      "data-presence-user",
      ownerLogin.user.id,
    );
    await expect(caret).toHaveAttribute("data-presence-name", OWNER_NAME);
    await expect(pageB.getByTestId("flow-presence-list")).toContainText(
      OWNER_NAME,
    );
    await pageB.screenshot({ path: shot("b-sees-a-presence") });

    // 2. B edits three times offline; all three reach A after reconnecting.
    await editorB.click();
    await pageB.keyboard.press("Control+End");
    await contextB.setOffline(true);
    await expect(indicatorB).toHaveAttribute("data-state", "offline");
    for (const text of [" b1", " b2", " b3"]) {
      await pageB.keyboard.insertText(text);
    }
    await expect(indicatorB).toHaveAttribute("data-pending", "3");
    await expect(indicatorB).toContainText("3 pending");
    await pageB.screenshot({ path: shot("b-offline-3-pending") });
    await expect(editorA).not.toContainText("b1");
    await contextB.setOffline(false);
    await expect(indicatorB).toHaveAttribute("data-pending", "0", {
      timeout: 20_000,
    });
    await expect(editorA).toContainText("Hello from A b1 b2 b3");

    // 3. B edits offline again; the owner lowers B to view meanwhile.
    await editorB.click();
    await pageB.keyboard.press("Control+End");
    await contextB.setOffline(true);
    await expect(indicatorB).toHaveAttribute("data-state", "offline");
    await pageB.keyboard.insertText(" x1");
    await pageB.keyboard.insertText(" x2");
    await expect(indicatorB).toHaveAttribute("data-pending", "2");
    await unwrap(
      await request.put(`/api/v1/flow/objects/${objectId}/grants`, {
        headers: auth,
        data: {
          grants: [
            {
              principal_kind: "user",
              principal_id: memberLogin.user.id,
              level: "view",
            },
          ],
          confirm_self_lockout: false,
          dry_run: false,
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    await unwrap(
      await request.put(`/api/v1/flow/objects/${objectId}/inheritance`, {
        headers: auth,
        data: {
          inherit_from_parent: false,
          confirm_self_lockout: false,
          dry_run: false,
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    const grantsB = await unwrap<{ effective_level: string }>(
      await request.get(`/api/v1/flow/objects/${objectId}/grants`, {
        headers: { Authorization: `Bearer ${memberLogin.tokens.access_token}` },
      }),
    );
    expect(grantsB.effective_level).toBe("view");
    await contextB.setOffline(false);

    const draft = pageB.getByTestId("flow-recovery-draft");
    await expect(draft).toBeVisible({ timeout: 20_000 });
    await expect(draft).toHaveAttribute("data-count", "2");
    await expect(pageB.getByTestId("flow-recovery-entry")).toHaveCount(2);
    await expect(
      pageB.getByTestId("flow-recovery-entry").first(),
    ).toHaveAttribute("data-reason", "policy_rejected");
    await expect(indicatorB).toHaveAttribute("data-state", "read_only");
    await expect(pageB.getByTestId("flow-canvas")).toHaveAttribute(
      "data-read-only",
      "true",
    );
    await expect(editorB).toHaveAttribute("contenteditable", "false");
    await expect(editorB).not.toContainText("x1");
    await pageB.screenshot({ path: shot("b-read-only-recovery-draft") });

    const [download] = await Promise.all([
      pageB.waitForEvent("download"),
      pageB.getByTestId("flow-recovery-download").click(),
    ]);
    expect(download.suggestedFilename()).toBe(
      `${objectId}-recovery-draft.json`,
    );
    const downloaded = JSON.parse(
      readFileSync(await download.path(), "utf8"),
    ) as {
      schema: string;
      object_id: string;
      entries: Array<{
        update_id: string;
        bytes_base64: string;
        reason: { code: string };
      }>;
    };
    expect(downloaded.schema).toBe("sylvode.flow.recovery-draft.v1");
    expect(downloaded.object_id).toBe(objectId);
    expect(downloaded.entries).toHaveLength(2);
    for (const entry of downloaded.entries) {
      expect(entry.reason.code).toBe("policy_rejected");
      expect(entry.bytes_base64.length).toBeGreaterThan(0);
    }
    await expect(editorA).not.toContainText("x1");

    // Typing into B's read-only canvas changes nothing.
    await editorB.click();
    await pageB.keyboard.insertText(" nope");
    await expect(editorB).not.toContainText("nope");

    // 4. A keeps editing after the grant change; read-only B still receives it live.
    const socketsBeforeAfterEdit = socketsA;
    await typeAtEnd(pageA, " after");
    await expect(editorB).toContainText("Hello from A b1 b2 b3 after", {
      timeout: 20_000,
    });
    await expect(pageA.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-pending",
      "0",
    );
    await expect(pageA.getByTestId("flow-recovery-draft")).toHaveCount(0);
    expect(socketsA).toBe(socketsBeforeAfterEdit);

    // The server's accepted state, as a fresh load sees it: everything accepted, nothing drafted.
    const fresh = await contextA.newPage();
    await fresh.goto(url);
    const freshEditor = fresh.locator(".flow-canvas .ProseMirror");
    await expect(freshEditor).toContainText("Hello from A b1 b2 b3 after");
    await expect(freshEditor).not.toContainText("x1");
    await expect(freshEditor).not.toContainText("x2");
    await fresh.screenshot({ path: shot("fresh-load-accepted-state") });
  } finally {
    await contextA.close();
    await contextB.close();
  }
});

function shot(name: string): string {
  const dir = process.env.E2E_SCREENSHOT_DIR ?? "test-results";
  return `${dir}/flow-multi-user-${name}.png`;
}
