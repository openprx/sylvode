import { expect, test, type Page } from '@playwright/test';

const email = process.env.TEST_EMAIL ?? 'demo@sylvode.local';
const password = process.env.TEST_PASSWORD ?? 'SylvodeDemo123!';
const workspaceId = process.env.OPENPR_WORKSPACE_ID ?? '07f6e023-6b0a-425c-bdac-442b5d36cd0c';
const projectId = process.env.OPENPR_PROJECT_ID ?? '8f7f7726-948e-4ea4-b149-06f25753b525';
const codeTaskFormId =
	process.env.OPENPR_CODE_TASK_FORM_ID ?? 'db4d73cc-9639-44b9-9875-21be69b0831f';

type LoginData = {
	tokens: { access_token: string; refresh_token: string };
	user: unknown;
};

async function login(page: Page): Promise<LoginData> {
	const response = await page.request.post('/api/v1/auth/login', {
		data: { email, password }
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	const loginData = body.data as LoginData;

	await page.addInitScript(
		({ accessToken, refreshToken, authUser }) => {
			localStorage.setItem('auth_token', accessToken);
			localStorage.setItem('refresh_token', refreshToken);
			localStorage.setItem('auth_user', JSON.stringify(authUser));
			localStorage.setItem('locale', 'zh');
		},
		{
			accessToken: loginData.tokens.access_token,
			refreshToken: loginData.tokens.refresh_token,
			authUser: loginData.user
		}
	);

	return loginData;
}

async function createRecord(page: Page, token: string, suffix: string) {
	const repo = `qa-detail-${suffix}`;
	const branch = `feature/${suffix}`;
	const response = await page.request.post(`/api/v1/forms/${codeTaskFormId}/records`, {
		headers: { Authorization: `Bearer ${token}` },
		data: {
			values: {
				repo,
				directory: '/srv/repos/sylvode',
				branch,
				ci_status: 'passing',
				risk: 'medium',
				verification: 'Initial detail/edit/delete fixture'
			},
			source: { type: 'playwright' }
		}
	});
	expect(response.ok()).toBe(true);
	const body = await response.json();
	expect(body.code).toBe(0);
	return {
		id: body.data.id as string,
		title: body.data.title as string,
		repo,
		branch
	};
}

function formsPath() {
	return `/workspace/${workspaceId}/projects/${projectId}/forms?form=${codeTaskFormId}`;
}

test.describe('Universal forms / record side panel', () => {
	test('opens a record beside the list, moves with the keyboard and closes with Back', async ({
		page
	}) => {
		const loginData = await login(page);
		const suffix = Date.now().toString(36);
		const first = await createRecord(page, loginData.tokens.access_token, `${suffix}a`);
		const second = await createRecord(page, loginData.tokens.access_token, `${suffix}b`);

		try {
			await page.goto(formsPath());
			await expect(page.locator('[data-view-renderer="grid"]')).toBeVisible();
			await expect(page.locator('[data-view-switcher]')).toContainText('全部记录');

			// A plain cell click opens the panel; the list stays visible beside it.
			const row = page.locator(`[data-record-row="${first.id}"]`);
			await row.locator('td').nth(1).click();
			const panel = page.locator('[data-record-panel]');
			await expect(panel).toHaveAttribute('data-record-panel', first.id);
			await expect(row).toHaveAttribute('aria-current', 'true');
			await expect(page.locator('[data-record-list-panel]')).toBeVisible();
			await expect(panel.locator('input, textarea, select')).toHaveCount(0);
			await expect(page).toHaveURL(new RegExp(`record=${first.id}`));
			expect(await page.locator('iframe').count()).toBe(0);

			// Back closes the panel and stays on the list.
			await page.goBack();
			await expect(panel).toHaveCount(0);
			await expect(page).not.toHaveURL(/record=/);

			// Enter opens, arrows move without new history entries, Esc returns focus to the row.
			const firstLink = page.locator(`[data-record-title-link="${first.id}"]`);
			await firstLink.focus();
			await page.keyboard.press('Enter');
			await expect(panel).toHaveAttribute('data-record-panel', first.id);
			const historyLength = await page.evaluate(() => history.length);
			const order = await page
				.locator('[data-record-row]')
				.evaluateAll((rows) => rows.map((item) => item.getAttribute('data-record-row')));
			const next = order[order.indexOf(first.id) + 1];
			await page.keyboard.press('ArrowDown');
			await expect(panel).toHaveAttribute('data-record-panel', next ?? first.id);
			expect(await page.evaluate(() => history.length)).toBe(historyLength);
			await page.keyboard.press('Escape');
			await expect(panel).toHaveCount(0);
			await expect(page.locator(`[data-record-title-link="${next ?? first.id}"]`)).toBeFocused();

			// Reloading a ?record= URL reopens the panel.
			await page.goto(`${formsPath()}&record=${second.id}`);
			await expect(panel).toHaveAttribute('data-record-panel', second.id);

			// Narrow screens get a modal sheet.
			await page.setViewportSize({ width: 390, height: 844 });
			await expect(panel).toHaveAttribute('data-record-panel-mode', 'sheet');
			await expect(panel).toHaveAttribute('role', 'dialog');
			await expect(panel).toHaveAttribute('aria-modal', 'true');
		} finally {
			for (const record of [first, second]) {
				await page.request.delete(`/api/v1/form-records/${record.id}`, {
					headers: { Authorization: `Bearer ${loginData.tokens.access_token}` }
				});
			}
		}
	});
});
