import { readFile } from "node:fs/promises";
import {
  test,
  expect,
  type APIRequestContext,
  type BrowserContext,
  type Page,
} from "@playwright/test";

type Session = {
  tokens: { access_token: string; refresh_token: string };
  user: { id: string; name: string };
};
type ObjectView = {
  id: string;
  scope_id: string;
  workspace_id?: string;
  document_id: string;
  semantic_content: unknown;
};
async function unwrap<T>(
  response: Pick<
    Awaited<ReturnType<APIRequestContext["get"]>>,
    "ok" | "url" | "status" | "json"
  >,
): Promise<T> {
  expect(response.ok(), `${response.url()} HTTP ${response.status()}`).toBe(
    true,
  );
  const body = await response.json();
  expect(body.code, body.message).toBe(0);
  return body.data as T;
}
async function signIn(context: BrowserContext, session: Session) {
  await context.addInitScript(({ tokens, user }) => {
    localStorage.setItem("auth_token", tokens.access_token);
    localStorage.setItem("refresh_token", tokens.refresh_token);
    localStorage.setItem("auth_user", JSON.stringify(user));
    localStorage.setItem("locale", "en");
  }, session);
  return context.newPage();
}

async function expectTitleVisible(page: Page) {
  const title = page.getByRole("textbox", { name: "Page title", exact: true });
  await expect
    .poll(() =>
      title.evaluate((element) => ({
        vertical: element.scrollHeight <= element.clientHeight + 1,
        horizontal: element.scrollWidth <= element.clientWidth + 1,
      })),
    )
    .toEqual({ vertical: true, horizontal: true });
}

async function downloadUiArtifact(
  page: Page,
  request: APIRequestContext,
  objectId: string,
  auth: Record<string, string>,
  format: "json" | "csv",
  expectedText: string,
) {
  const panel = page.getByTestId("flow-panel-export");
  await panel
    .getByRole("combobox", { name: "Format", exact: true })
    .selectOption(format);
  const receiptPromise = page.waitForResponse(
    (response) =>
      response.url().endsWith(`/api/v2/flow/objects/${objectId}/exports`) &&
      response.request().method() === "POST",
  );
  await panel.getByRole("button", { name: "Export", exact: true }).click();
  const response = await receiptPromise;
  const receipt = await unwrap<{ job_id: string }>(response);
  await expect(panel.getByTestId("flow-panel-export-job")).toHaveAttribute(
    "data-job-id",
    receipt.job_id,
  );
  await expect(panel.getByTestId("flow-panel-export-status")).toHaveText(
    "Completed",
  );
  const downloaded = page.waitForEvent("download");
  await panel.getByRole("button", { name: "Download", exact: true }).click();
  const file = await downloaded;
  expect(file.suggestedFilename().endsWith("." + format)).toBe(true);
  const path = test.info().outputPath(`personal-owner-ui-${format}.${format}`);
  await file.saveAs(path);
  const saved = await readFile(path);
  expect(saved.toString("utf8")).toContain(expectedText);
  if (format === "json")
    expect(() => JSON.parse(saved.toString("utf8"))).not.toThrow();
  const raw = await request.get(
    `/api/v2/flow/exports/${receipt.job_id}/artifact`,
    { headers: auth },
  );
  expect(raw.ok()).toBe(true);
  expect(
    saved.equals(await raw.body()),
    "UI preserves exact authenticated artifact bytes",
  ).toBe(true);
  await test.info().attach(`ui-${format}-artifact-proof.json`, {
    body: JSON.stringify({
      job_id: receipt.job_id,
      format,
      bytes: saved.byteLength,
      idempotency_key: response.request().postDataJSON().idempotency_key,
    }),
    contentType: "application/json",
  });
  return response.request().postDataJSON().idempotency_key as string;
}

test.beforeEach(async ({ browser }, info) => {
  await info.attach("browser-identity.json", {
    body: JSON.stringify({
      executable: info.project.use.launchOptions?.executablePath,
      version: browser.version(),
      project: info.project.name,
      headless: info.project.use.headless,
    }),
    contentType: "application/json",
  });
});

async function fixtures(request: APIRequestContext) {
  const run = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
  const password = `Flow-${run}-1!`;
  const adminAccount =
    process.env.ADMIN_EMAIL && process.env.ADMIN_PASSWORD
      ? { email: process.env.ADMIN_EMAIL, password: process.env.ADMIN_PASSWORD }
      : { email: `flow-admin-${run}@e2e.sylvode.test`, password };
  if (!process.env.ADMIN_EMAIL)
    await unwrap(
      await request.post("/api/v1/auth/register", {
        data: { ...adminAccount, name: "Flow admin" },
      }),
    );
  const admin = await unwrap<Session>(
    await request.post("/api/v1/auth/login", { data: adminAccount }),
  );
  async function account(label: string) {
    const credentials = {
      email: `flow-${label}-${run}@e2e.sylvode.test`,
      password,
    };
    await unwrap(
      await request.post("/api/v1/auth/register", {
        headers: { Authorization: `Bearer ${admin.tokens.access_token}` },
        data: { ...credentials, name: `Flow ${label}` },
      }),
    );
    return unwrap<Session>(
      await request.post("/api/v1/auth/login", { data: credentials }),
    );
  }
  const owner = await account("personal");
  const visitor = await account("team");
  const auth = { Authorization: `Bearer ${owner.tokens.access_token}` };
  const visitorAuth = {
    Authorization: `Bearer ${visitor.tokens.access_token}`,
  };
  const workspaceList = async () =>
    unwrap<{ items: unknown[] }>(
      await request.get("/api/v1/workspaces", { headers: auth }),
    );
  expect((await workspaceList()).items).toEqual([]);
  return { run, owner, visitor, auth, visitorAuth, workspaceList };
}

test("ordinary global user creates, reopens, searches and exports personal Flow with zero workspaces", async ({
  browser,
  request,
}) => {
  test.setTimeout(180_000);
  const { run, owner, auth, workspaceList } = await fixtures(request);
  const ownerContext = await browser.newContext();
  try {
    const page = await signIn(ownerContext, owner);
    await page.goto("/my-work");
    await expect(page.getByTestId("global-flow-link")).toBeVisible();
    await page.getByTestId("global-flow-link").click();
    await expect(page).toHaveURL(/\/flow$/);
    await page.getByTestId("flow-nav-new").click();
    const createResponse = page.waitForResponse(
      (response) =>
        response.url().endsWith("/api/v2/flow/objects") &&
        response.request().method() === "POST",
    );
    await page.getByRole("menuitem", { name: "New Page", exact: true }).click();
    const created = await unwrap<{ object: ObjectView }>(await createResponse);
    const source = created.object;
    expect(source.scope_id).toBeTruthy();
    expect(source.workspace_id).toBeFalsy();
    const editor = page.locator(".flow-canvas .ProseMirror");
    await expect(editor).toBeVisible();
    await page
      .getByRole("textbox", { name: "Page title", exact: true })
      .fill(`Personal page ${run}`);
    await editor.click();
    await page.keyboard.type(`Independent prose ${run}`);
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "saved",
    );
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-pending",
      "0",
    );
    await page.reload();
    await expect(editor).toContainText(`Independent prose ${run}`);
    await expect(
      page.getByRole("textbox", { name: "Page title", exact: true }),
    ).toHaveValue(`Personal page ${run}`);
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "local",
    );
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-pending",
      "0",
    );
    await editor.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" Reopened live");
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "saved",
    );
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-pending",
      "0",
    );
    await ownerContext.setOffline(true);
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "offline",
    );
    await editor.click();
    await page.keyboard.press("End");
    await page.keyboard.type(" Offline buffered");
    await expect(page.getByTestId("flow-sync-indicator")).not.toHaveAttribute(
      "data-pending",
      "0",
    );
    await ownerContext.setOffline(false);
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "saved",
    );
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-pending",
      "0",
    );
    const object = await unwrap<ObjectView>(
      await request.get(`/api/v2/flow/objects/${source.id}`, { headers: auth }),
    );
    expect(object.document_id).toBe(source.document_id);
    expect(JSON.stringify(object.semantic_content)).toContain(
      `Independent prose ${run} Reopened live Offline buffered`,
    );
    const search = await unwrap<{ items: unknown[] }>(
      await request.get(
        `/api/v2/flow/scopes/${source.scope_id}/search?q=${encodeURIComponent(run)}&all_visible=true&freshness=require_current`,
        { headers: auth },
      ),
    );
    expect(JSON.stringify(search.items)).toContain(source.id);
    expect((await workspaceList()).items).toEqual([]);
    await expectTitleVisible(page);
    await page.screenshot({
      path: test.info().outputPath("personal-zero-workspaces-wide.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(editor).toBeVisible();
    await page.getByTestId("flow-nav-toggle").click();
    await expect(page.getByTestId("flow-nav-drawer")).toBeVisible();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("flow-nav-toggle")).toBeFocused();
    await expect(page.getByTestId("flow-nav-drawer")).not.toBeVisible();
    await expectTitleVisible(page);
    await page.screenshot({
      path: test.info().outputPath("personal-zero-workspaces-mobile.png"),
      fullPage: true,
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    await test.info().attach("personal-scope-fixture.json", {
      body: JSON.stringify({
        user_id: owner.user.id,
        scope_id: source.scope_id,
        object_id: source.id,
        document_id: source.document_id,
        workspace_count: (await workspaceList()).items.length,
      }),
      contentType: "application/json",
    });
    const exportReceipt = await unwrap<{ job_id: string }>(
      await request.post(`/api/v2/flow/objects/${source.id}/exports`, {
        headers: auth,
        data: {
          format: "markdown",
          include_history: false,
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    let exported: { status: string; download_url?: string } = {
      status: "pending",
    };
    await expect
      .poll(
        async () => {
          exported = await unwrap<{ status: string; download_url?: string }>(
            await request.get(`/api/v2/flow/exports/${exportReceipt.job_id}`, {
              headers: auth,
            }),
          );
          return exported.status;
        },
        { timeout: 30_000 },
      )
      .toBe("completed");
    expect(exported!.download_url).toBeTruthy();
    const artifact = await request.get(exported!.download_url!, {
      headers: auth,
    });
    expect(artifact.ok()).toBe(true);
    expect(await artifact.text()).toContain(
      `Independent prose ${run} Reopened live Offline buffered`,
    );

    const expectedText = `Independent prose ${run} Reopened live Offline buffered`;
    const jsonKey = await downloadUiArtifact(
      page,
      request,
      source.id,
      auth,
      "json",
      expectedText,
    );
    const csvKey = await downloadUiArtifact(
      page,
      request,
      source.id,
      auth,
      "csv",
      expectedText,
    );
    expect(csvKey).not.toBe(jsonKey);
  } finally {
    await ownerContext.close();
  }
});

test("a team mount edits the canonical personal source, denies siblings and revokes without deleting source", async ({
  browser,
  request,
}) => {
  test.setTimeout(180_000);
  const { run, owner, visitor, auth, visitorAuth } = await fixtures(request);
  const ownerContext = await browser.newContext();
  const visitorContext = await browser.newContext();
  try {
    const home = await unwrap<{ scope: { id: string } }>(
      await request.get("/api/v2/flow/home", { headers: auth }),
    );
    const created = await unwrap<{ object: ObjectView }>(
      await request.post("/api/v2/flow/objects", {
        headers: auth,
        data: {
          scope_id: home.scope.id,
          object_type: "page",
          title: "Mount source",
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    const source = created.object;
    const page = await signIn(ownerContext, owner);
    await page.goto(`/flow/${source.id}`);
    const editor = page.locator(".flow-canvas .ProseMirror");
    await expect(editor).toBeVisible();
    await editor.click();
    await page.keyboard.type(`Independent prose ${run}`);
    await expect(page.getByTestId("flow-sync-indicator")).toHaveAttribute(
      "data-state",
      "saved",
    );
    const sibling = await unwrap<{ object: ObjectView }>(
      await request.post("/api/v2/flow/objects", {
        headers: auth,
        data: {
          scope_id: source.scope_id,
          object_type: "page",
          title: "Private sibling",
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    const team = await unwrap<{ id: string }>(
      await request.post("/api/v1/workspaces", {
        headers: visitorAuth,
        data: { name: `Team ${run}`, slug: `team-${run}` },
      }),
    );
    await unwrap(
      await request.post(`/api/v1/workspaces/${team.id}/members`, {
        headers: visitorAuth,
        data: { user_id: owner.user.id, role: "admin" },
      }),
    );
    const mount = await unwrap<{ id: string }>(
      await request.post(`/api/v2/flow/objects/${source.id}/mounts`, {
        headers: auth,
        data: {
          destination_workspace_id: team.id,
          level: "edit",
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    const visitorPage = await signIn(visitorContext, visitor);
    const ticketBodies: unknown[] = [];
    visitorPage.on("request", (request) => {
      if (request.url().endsWith("/api/v2/flow/collab/tickets"))
        ticketBodies.push(request.postDataJSON());
    });
    await visitorPage.goto(`/flow/${source.id}`);
    const visitorEditor = visitorPage.locator(".flow-canvas .ProseMirror");
    await expect(visitorEditor).toContainText(`Independent prose ${run}`);
    await expect(visitorEditor).toHaveAttribute("contenteditable", "true");
    expect(
      ticketBodies.some(
        (body) => (body as { scope_id?: string }).scope_id === source.scope_id,
      ),
    ).toBe(true);
    await visitorEditor.click();
    await visitorPage.keyboard.press("End");
    await visitorPage.keyboard.type(" Team contribution");
    await expect(editor).toContainText("Team contribution");
    const hiddenSibling = await request.get(
      `/api/v2/flow/objects/${sibling.object.id}`,
      { headers: visitorAuth },
    );
    const denial = await hiddenSibling.json();
    expect(["forbidden", "not_found"]).toContain(denial.error_code);
    expect(denial.data ?? null).toBeNull();
    await visitorPage.screenshot({
      path: test.info().outputPath("mounted-canonical-source.png"),
      fullPage: true,
    });
    await unwrap(
      await request.delete(
        `/api/v2/flow/objects/${source.id}/mounts/${mount.id}`,
        { headers: auth },
      ),
    );
    await expect(visitorEditor).toHaveAttribute("contenteditable", "false");
    const preserved = await unwrap<ObjectView>(
      await request.get(`/api/v2/flow/objects/${source.id}`, { headers: auth }),
    );
    expect(preserved.scope_id).toBe(source.scope_id);
    expect(preserved.document_id).toBe(source.document_id);
  } finally {
    await ownerContext.close();
    await visitorContext.close();
  }
});

test("personal and directly shared collections keep typed record operations and canonical identities without workspaces", async ({
  browser,
  request,
}) => {
  test.setTimeout(180_000);
  const { run, owner, visitor, auth, visitorAuth, workspaceList } =
    await fixtures(request);
  const home = await unwrap<{ scope: { id: string } }>(
    await request.get("/api/v2/flow/home", { headers: auth }),
  );
  const ownerContext = await browser.newContext();
  const visitorContext = await browser.newContext();
  const ticketDocuments: string[] = [];
  const captureTickets = (page: Page) =>
    page.on("request", (req) => {
      if (req.url().endsWith("/api/v2/flow/collab/tickets"))
        ticketDocuments.push(req.postDataJSON().document_id);
    });
  try {
    const page = await signIn(ownerContext, owner);
    captureTickets(page);
    await page.goto("/flow");
    await page.getByTestId("flow-nav-new").click();
    await page
      .getByRole("menuitem", { name: "New Collection", exact: true })
      .click();
    await page
      .getByTestId("flow-create-collection-name")
      .fill(`Personal collection ${run}`);
    await page.getByTestId("flow-create-collection-field").fill("Name");
    const creation = page.waitForResponse(
      (response) =>
        response.url().endsWith("/api/v2/flow/objects") &&
        response.request().method() === "POST",
    );
    await page.getByTestId("flow-create-collection-submit").click();
    const createdResponse = await creation;
    const created = await unwrap<{ object: ObjectView }>(createdResponse);
    const collection = created.object;
    const creationBody = createdResponse.request().postDataJSON();
    expect(creationBody.scope_id).toBe(home.scope.id);
    expect(creationBody.workspace_id).toBeUndefined();
    expect(creationBody.initial_fields[0].field_type).toBe("text");
    const fieldId = creationBody.initial_fields[0].field_id;
    await expect(page.getByTestId("collection-table")).toBeVisible();
    const recordCreation = page.waitForResponse(
      (response) =>
        response
          .url()
          .endsWith(`/api/v2/flow/collections/${collection.id}/records`) &&
        response.request().method() === "POST",
    );
    await page.getByTestId("collection-new-record").click();
    const record = (await unwrap<{ object: ObjectView }>(await recordCreation))
      .object;
    const row = page.locator(
      `[data-testid="collection-row"][data-record-id="${record.id}"]`,
    );
    const cell = row.locator(`[data-field-id="${fieldId}"]`);
    await cell.dblclick();
    const patch = page.waitForResponse(
      (response) =>
        response
          .url()
          .endsWith(`/api/v2/flow/objects/${collection.id}/commands`) &&
        response.request().method() === "POST",
    );
    await cell.getByRole("textbox").fill(`Typed owner ${run}`);
    await cell.getByRole("textbox").press("Enter");
    const accepted = await patch;
    await unwrap(accepted);
    expect(accepted.request().postDataJSON().command.type).toBe("record_patch");
    await expect(cell).toContainText(`Typed owner ${run}`);
    await page.goto(`/flow/${record.id}`);
    await expect(page.getByTestId("flow-record-route")).toBeVisible();
    await expect(
      page
        .getByTestId("record-panel")
        .getByRole("textbox", { name: "Name", exact: true }),
    ).toHaveValue(`Typed owner ${run}`);
    const before = await unwrap<ObjectView>(
      await request.get(`/api/v2/flow/objects/${record.id}`, { headers: auth }),
    );
    expect(before.scope_id).toBe(home.scope.id);
    expect(before.document_id).toBe(record.document_id);
    await unwrap(
      await request.put(`/api/v2/flow/objects/${collection.id}/grants`, {
        headers: auth,
        data: {
          grants: [
            {
              principal_kind: "user",
              principal_id: visitor.user.id,
              level: "edit",
            },
          ],
          dry_run: false,
          confirm_self_lockout: false,
          idempotency_key: crypto.randomUUID(),
        },
      }),
    );
    const guestPage = await signIn(visitorContext, visitor);
    captureTickets(guestPage);
    await guestPage.goto(`/flow/${collection.id}`);
    const guestRow = guestPage.locator(
      `[data-testid="collection-row"][data-record-id="${record.id}"]`,
    );
    const guestCell = guestRow.locator(`[data-field-id="${fieldId}"]`);
    await expect(guestCell).toContainText(`Typed owner ${run}`);
    await guestCell.dblclick();
    const guestPatch = guestPage.waitForResponse(
      (response) =>
        response
          .url()
          .endsWith(`/api/v2/flow/objects/${collection.id}/commands`) &&
        response.request().method() === "POST",
    );
    await guestCell.getByRole("textbox").fill(`Typed guest ${run}`);
    await guestCell.getByRole("textbox").press("Enter");
    await unwrap(await guestPatch);
    await page.getByTestId("collection-refresh").click();
    await expect(
      page
        .getByTestId("record-panel")
        .getByRole("textbox", { name: "Name", exact: true }),
    ).toHaveValue(`Typed guest ${run}`);
    const after = await unwrap<ObjectView>(
      await request.get(`/api/v2/flow/objects/${record.id}`, {
        headers: visitorAuth,
      }),
    );
    expect(after.scope_id).toBe(home.scope.id);
    expect(after.document_id).toBe(before.document_id);
    expect(ticketDocuments).not.toContain(collection.document_id);
    expect(ticketDocuments).not.toContain(record.document_id);
    expect((await workspaceList()).items).toEqual([]);
    const guestWorkspaces = await unwrap<{ items: unknown[] }>(
      await request.get("/api/v1/workspaces", { headers: visitorAuth }),
    );
    expect(guestWorkspaces.items).toEqual([]);
    await test.info().attach("typed-scope-fixture.json", {
      body: JSON.stringify({
        owner_id: owner.user.id,
        guest_id: visitor.user.id,
        scope_id: home.scope.id,
        collection_id: collection.id,
        collection_document_id: collection.document_id,
        record_id: record.id,
        record_document_id: record.document_id,
      }),
      contentType: "application/json",
    });
    await guestPage.screenshot({
      path: test.info().outputPath("personal-shared-typed-collection.png"),
      fullPage: true,
    });
  } finally {
    await ownerContext.close();
    await visitorContext.close();
  }
});
