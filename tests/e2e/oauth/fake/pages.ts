// The pages of the fake: just enough HTML and script to carry the data-testid values the suite
// drives, so the client's own browser logic is exercised without the real frontend.

const SHELL = (title: string, body: string, script: string): string =>
	`<!doctype html><html><head><meta charset="utf-8"><title>${title}</title></head><body>${body}<script>${script}</script></body></html>`;

const COMMON = `
const $ = (id) => document.querySelector('[data-testid="' + id + '"]');
async function api(method, path, body, headers) {
  const res = await fetch(path, { method, headers: Object.assign({ 'content-type': 'application/json' }, headers || {}), body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await res.text();
  let json = null; try { json = JSON.parse(text); } catch (e) { json = null; }
  return { status: res.status, json };
}
`;

export const AUTHORIZE_PAGE = SHELL(
	'Connect to Sylvode',
	'<div data-testid="consent-page" data-view="loading"><main id="main"></main></div>',
	`${COMMON}
const page = $('consent-page'); const main = document.getElementById('main');
let consent = null; let requestId = null; let mode = null; let workspaceId = null; let selected = new Set(); let projects = [];
function view(name) { page.setAttribute('data-view', name); }
function esc(s) { return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c])); }
async function start() {
  const params = new URLSearchParams(location.search);
  let body;
  if (params.has('request')) body = { request_id: params.get('request') };
  else {
    body = { response_type: params.get('response_type'), client_id: params.get('client_id'), redirect_uri: params.get('redirect_uri'), scope: params.get('scope'), state: params.get('state'), code_challenge: params.get('code_challenge'), code_challenge_method: params.get('code_challenge_method'), resource: params.getAll('resource'), device_name: params.get('device_name'), workspace_hint: params.get('workspace_hint') };
  }
  const res = await api('POST', '/api/v1/oauth/authorization-requests', body);
  const data = res.json && res.json.data;
  if (!data) { main.innerHTML = '<div data-testid="consent-failed">failed</div>'; view('failed'); return; }
  if (data.kind === 'error_page') { main.innerHTML = '<div data-testid="consent-error-page"><h1>This connection request is not valid</h1></div>'; view('invalid'); return; }
  if (data.kind === 'redirect') { location.assign(data.redirect_to); return; }
  requestId = data.request_id;
  history.replaceState(null, '', '/oauth/authorize?request=' + requestId);
  if (data.kind === 'login_required') { renderLogin(); return; }
  consent = data; renderConsent();
}
function renderLogin() {
  main.innerHTML = '<div data-testid="consent-login"><button data-testid="consent-login-button">Sign in</button><button data-testid="consent-no-account">I do not have an account</button></div>';
  view('login');
  $('consent-login-button').onclick = () => { location.assign('/auth/login?return_to=' + encodeURIComponent('/oauth/authorize?request=' + requestId)); };
  $('consent-no-account').onclick = async () => {
    const res = await api('POST', '/api/v1/oauth/authorization-requests/' + requestId + '/decision', { decision: 'no_account' });
    if (res.json && res.json.data && res.json.data.redirect_to) location.assign(res.json.data.redirect_to);
  };
}
function allowReady() {
  if (!workspaceId || !mode) return false;
  if (mode === 'selected' && selected.size === 0) return false;
  return true;
}
function refresh() { const allow = $('consent-allow'); if (allow) allow.disabled = !allowReady(); }
async function loadProjects() {
  const res = await api('GET', '/api/v1/workspaces/' + workspaceId + '/projects?per_page=100');
  projects = (res.json && res.json.data && res.json.data.items) || [];
  renderPicker();
}
function renderPicker() {
  const holder = document.getElementById('picker'); holder.innerHTML = '';
  if (mode !== 'selected') return;
  const box = document.createElement('div'); box.setAttribute('data-testid', 'consent-project-picker');
  const button = document.createElement('button'); button.setAttribute('data-testid', 'consent-project-select'); button.textContent = 'Choose projects';
  const list = document.createElement('div'); list.setAttribute('role', 'listbox'); list.hidden = true;
  for (const p of projects) {
    const opt = document.createElement('div'); opt.setAttribute('role', 'option'); opt.tabIndex = 0; opt.textContent = p.name + ' ' + p.key;
    opt.onclick = () => { if (selected.has(p.id)) selected.delete(p.id); else selected.add(p.id); refresh(); };
    list.appendChild(opt);
  }
  button.onclick = () => { list.hidden = !list.hidden; };
  document.addEventListener('keydown', (e) => { if (e.key === 'Escape') list.hidden = true; });
  box.appendChild(button); box.appendChild(list); holder.appendChild(box);
}
function renderConsent() {
  const ws = consent.workspaces.map((w) => '<label><input type="radio" name="ws" data-testid="consent-workspace-' + w.id + '" value="' + w.id + '"' + (w.eligible ? '' : ' disabled') + '> ' + esc(w.name) + '</label>').join('');
  const scopes = consent.scopes.map((s) => '<label><input type="checkbox" data-testid="consent-scope-' + s.name + '"' + (s.required ? ' checked disabled' : '') + '> ' + esc(s.name) + '</label>').join('');
  main.innerHTML = '<div data-testid="consent-form"><h1 data-testid="consent-title">Connect ' + esc(consent.client.display_name) + ' to Sylvode</h1>' +
    '<div>' + ws + '</div>' +
    '<div><label><input type="radio" name="mode" data-testid="consent-projects-all" value="all"> All</label><label><input type="radio" name="mode" data-testid="consent-projects-selected" value="selected"> Only these</label><div id="picker"></div></div>' +
    '<div>' + scopes + '</div>' +
    '<div data-testid="consent-error"></div>' +
    '<button data-testid="consent-deny">Cancel</button><button data-testid="consent-allow" disabled>Allow</button></div>';
  view('consent');
  document.querySelectorAll('input[name=ws]').forEach((el) => el.addEventListener('change', () => { workspaceId = el.value; selected = new Set(); refresh(); if (mode === 'selected') void loadProjects(); }));
  document.querySelectorAll('input[name=mode]').forEach((el) => el.addEventListener('change', () => { mode = el.value; refresh(); if (mode === 'selected' && workspaceId) void loadProjects(); else renderPicker(); }));
  $('consent-deny').onclick = () => decide({ decision: 'deny' });
  $('consent-allow').onclick = () => decide({ decision: 'allow', workspace_id: workspaceId, project_mode: mode, project_ids: [...selected], scopes: consent.scopes.filter((s) => s.required || $('consent-scope-' + s.name).checked).map((s) => s.name) });
}
async function decide(body) {
  const res = await api('POST', '/api/v1/oauth/authorization-requests/' + requestId + '/decision', body, { 'X-Sylvode-Consent': consent.consent_token });
  if (res.json && res.json.data && res.json.data.redirect_to) location.assign(res.json.data.redirect_to);
  else $('consent-error').textContent = (res.json && res.json.message) || 'refused';
}
void start();
`
);

export const LOGIN_PAGE = SHELL(
	'Sign in',
	'<form id="f"><input id="email" type="email"><input id="password" type="password"><button type="submit">Sign in</button></form>',
	`${COMMON}
document.getElementById('f').addEventListener('submit', async (e) => {
  e.preventDefault();
  const res = await api('POST', '/api/v1/auth/login', { email: document.getElementById('email').value, password: document.getElementById('password').value });
  if (res.json && res.json.code === 0) {
    const target = new URLSearchParams(location.search).get('return_to');
    location.assign(target && /^\\/oauth\\/authorize\\?request=[0-9a-f-]{36}$/.test(target) ? target : '/workspace');
  }
});
`
);

const GRANT_LIST = (kind: 'personal' | 'workspace', url: string): string =>
	SHELL(
		'Settings',
		`<section data-testid="connected-apps-${kind}"><div id="rows"></div></section><div id="dlg"></div>`,
		`${COMMON}
const KIND = '${kind}';
async function load() {
  const res = await api('GET', '${url}');
  const items = (res.json && res.json.data && res.json.data.items) || [];
  const rows = document.getElementById('rows'); rows.innerHTML = '';
  if (items.length === 0 && KIND === 'personal') rows.innerHTML = '<p data-testid="connected-apps-personal-empty">none</p>';
  for (const g of items) {
    const row = document.createElement('div'); row.setAttribute('data-testid', KIND === 'personal' ? 'connected-app-row' : 'workspace-app-row'); row.setAttribute('data-grant-id', g.grant_id);
    row.textContent = (g.client && g.client.display_name) + ' ';
    const b = document.createElement('button'); b.setAttribute('data-testid', KIND === 'personal' ? 'connected-app-disconnect' : 'workspace-app-disconnect'); b.textContent = 'Disconnect';
    b.onclick = () => confirmDisconnect(g.grant_id);
    row.appendChild(b); rows.appendChild(row);
  }
}
function confirmDisconnect(id) {
  const dlg = document.getElementById('dlg');
  dlg.innerHTML = '<div data-testid="confirm-dialog" role="alertdialog"><p>Disconnect?</p><button>Cancel</button><button id="ok">Disconnect Ramflux</button></div>';
  dlg.querySelector('button').onclick = () => { dlg.innerHTML = ''; };
  document.getElementById('ok').onclick = async () => {
    const path = KIND === 'personal' ? '/api/v1/me/connected-apps/' + id : location.pathname.replace('/settings', '') .replace('/workspace/', '/api/v1/workspaces/') + '/connected-apps/' + id;
    await api('DELETE', path);
    dlg.innerHTML = ''; await load();
  };
}
void load();
`
	);

export const SETTINGS_PAGE = GRANT_LIST('personal', '/api/v1/me/connected-apps');
export const workspaceSettingsPage = (workspaceId: string): string =>
	GRANT_LIST('workspace', `/api/v1/workspaces/${workspaceId}/connected-apps`);
