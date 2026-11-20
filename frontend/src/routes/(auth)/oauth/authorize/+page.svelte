<script lang="ts">
	/**
	 * Consent page of the authorization server (contract 6.1 and 14.1).
	 *
	 * It reads the authorization request from the address bar, hands it to the API, and shows what
	 * the API answers: an error page, a sign-in prompt, or the consent form. It never renders a
	 * query value, never builds a redirect address and navigates only to a `redirect_to` the API
	 * returned. The consent token lives in this component's memory and nowhere else.
	 */
	import '#lib/i18n/index.js';
	import { onMount, tick } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { goto, replaceState } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import { authApi } from '#lib/api/auth.js';
	import { projectsApi } from '#lib/api/projects.js';
	import { authStore } from '#lib/stores/auth.js';
	import {
		createAuthorizationRequest,
		failureKey,
		parseTooManyGrants,
		submitDecision,
		type AuthorizationOutcome,
		type AuthorizationRequestInput,
		type ClientDisplay,
		type ConsentData,
		type Decision,
		type OAuthFailure,
		type TooManyGrants
	} from '#lib/api/oauth.js';
	import { parseAuthorizeQuery } from '#lib/oauth/consent-request.js';
	import {
		allowBody,
		evaluateConsent,
		initialSelection,
		isNavigableRedirect,
		markIneligible,
		selectWorkspace,
		type ConsentSelection,
		type ProjectMode,
		type ProjectsStatus,
		type WorkspaceChoice
	} from '#lib/oauth/consent-state.js';
	import { groupScopes, isKnownScope } from '#lib/oauth/grant-summary.js';
	import { CONSENT_PATH, loginUrlForRequest } from '#lib/oauth/return-to.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import MultiSelect from '#lib/ui/multi-select/MultiSelect.svelte';
	import Radio from '#lib/ui/radio/Radio.svelte';
	import RadioGroup from '#lib/ui/radio/RadioGroup.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import Skeleton from '#lib/ui/skeleton/Skeleton.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';

	type View =
		| { kind: 'loading' }
		| { kind: 'framed' }
		| { kind: 'invalid' }
		| { kind: 'expired'; client: string | null }
		| { kind: 'failed'; messageKey: string }
		| { kind: 'login'; requestId: string; client: ClientDisplay }
		| { kind: 'consent' }
		| { kind: 'tooMany'; info: TooManyGrants | null }
		| { kind: 'done'; client: string };

	type Busy = 'allow' | 'deny' | 'no_account' | 'signout' | null;

	let view = $state<View>({ kind: 'loading' });
	let consent = $state<ConsentData | null>(null);
	let token = $state<string | null>(null);
	let workspaces = $state<WorkspaceChoice[]>([]);
	let selection = $state<ConsentSelection>({
		workspaceId: null,
		projectMode: null,
		projectIds: [],
		optionalScopes: new Set()
	});
	let projects = $state<Array<{ id: string; key: string; name: string }>>([]);
	let projectsStatus = $state<ProjectsStatus>('idle');
	let clientAvailable = $state(true);
	let busy = $state<Busy>(null);
	let errorKey = $state<string | null>(null);
	let lastInput: AuthorizationRequestInput | null = null;
	let refreshTried = false;
	let projectsLoad = 0;

	let errorBox = $state<HTMLElement | null>(null);
	let heading = $state<HTMLElement | null>(null);

	const clientName = $derived(
		consent?.client.displayName ?? (view.kind === 'login' ? view.client.displayName : '')
	);
	const result = $derived(
		consent
			? evaluateConsent({
					workspaces,
					scopes: consent.scopes,
					resources: consent.resources,
					selection,
					clientAvailable,
					projectsStatus
				})
			: null
	);
	const scopeGroups = $derived(groupScopes(consent?.scopes ?? []));
	const projectOptions = $derived(
		projects.map((project) => ({ value: project.id, label: project.name, meta: project.key }))
	);

	function client(): string {
		return clientName || '';
	}

	function scopeLabel(name: string): string {
		return isKnownScope(name)
			? $t(`oauth.scope.${name}`, { values: { client: client() } })
			: $t('oauth.scope.unknown', { values: { name } });
	}

	// --- loading ----------------------------------------------------------------------------

	onMount(() => {
		if (window.top !== window.self) {
			view = { kind: 'framed' };
			return;
		}
		const parsed = parseAuthorizeQuery(window.location.search);
		if (parsed.kind === 'invalid') {
			view = { kind: 'invalid' };
		} else if (parsed.kind === 'expired') {
			view = { kind: 'expired', client: null };
		} else {
			void load(parsed);
		}
	});

	async function load(input: AuthorizationRequestInput, notice: string | null = null) {
		lastInput = input;
		view = { kind: 'loading' };
		errorKey = null;
		let answered = await createAuthorizationRequest(input);
		// A stale access token reads as "not signed in": one refresh may restore the session.
		if (
			answered.ok &&
			answered.data.kind === 'login_required' &&
			!refreshTried &&
			(apiClient.getToken() !== null || apiClient.getRefreshToken() !== null)
		) {
			refreshTried = true;
			if (await apiClient.ensureFreshAccessToken()) {
				answered = await createAuthorizationRequest({
					kind: 'reload',
					requestId: answered.data.requestId
				});
			}
		}
		if (!answered.ok) {
			view = { kind: 'failed', messageKey: failureKey(answered.failure) };
			await focusHeading();
			return;
		}
		apply(answered.data);
		if (notice) errorKey = notice;
		else await focusHeading();
	}

	function rememberRequest(requestId: string) {
		// The address keeps only the stored request's id, so a reload re-reads it (the original
		// query is not replayed) and nothing secret is ever in the URL.
		const target = `${CONSENT_PATH}?request=${requestId}`;
		if (window.location.pathname + window.location.search !== target) {
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- fixed same-origin path
			replaceState(target, {});
		}
	}

	function apply(outcome: AuthorizationOutcome) {
		switch (outcome.kind) {
			case 'error_page':
				view =
					outcome.error === 'request_expired'
						? { kind: 'expired', client: null }
						: { kind: 'invalid' };
				return;
			case 'redirect':
				navigate(outcome.redirectTo, null);
				return;
			case 'login_required':
				rememberRequest(outcome.requestId);
				consent = null;
				token = null;
				view = { kind: 'login', requestId: outcome.requestId, client: outcome.client };
				return;
			case 'consent':
				rememberRequest(outcome.consent.requestId);
				consent = outcome.consent;
				token = outcome.consent.consentToken;
				workspaces = outcome.consent.workspaces;
				selection = initialSelection(workspaces, outcome.consent.preselectedWorkspaceId);
				projects = [];
				projectsStatus = 'idle';
				clientAvailable = true;
				busy = null;
				view = { kind: 'consent' };
				return;
		}
	}

	async function focusHeading() {
		await tick();
		heading?.focus();
	}

	// --- projects ---------------------------------------------------------------------------

	async function loadProjects(workspaceId: string) {
		const mine = ++projectsLoad;
		projectsStatus = 'loading';
		projects = [];
		const listed = await projectsApi.list(workspaceId);
		if (mine !== projectsLoad) return;
		if (listed.code !== 0 || !listed.data) {
			projectsStatus = 'failed';
			return;
		}
		projects = listed.data.items.map((project) => ({
			id: project.id,
			key: project.key,
			name: project.name
		}));
		projectsStatus = 'ready';
	}

	function chooseWorkspace(id: string) {
		selection = selectWorkspace(selection, id);
		projects = [];
		projectsStatus = 'idle';
		if (selection.projectMode === 'selected') void loadProjects(id);
	}

	function chooseMode(mode: string) {
		if (mode !== 'all' && mode !== 'selected') return;
		const next: ProjectMode = mode;
		selection = { ...selection, projectMode: next };
		if (next === 'selected' && selection.workspaceId && projectsStatus === 'idle') {
			void loadProjects(selection.workspaceId);
		}
	}

	function toggleOptional(name: string, on: boolean) {
		const next = new SvelteSet(selection.optionalScopes);
		if (on) next.add(name);
		else next.delete(name);
		selection = { ...selection, optionalScopes: next };
	}

	// --- decisions --------------------------------------------------------------------------

	function navigate(redirectTo: string, name: string | null) {
		if (!isNavigableRedirect(redirectTo)) {
			errorKey = 'oauth.errors.generic';
			busy = null;
			return;
		}
		view = { kind: 'done', client: name ?? client() };
		token = null;
		window.location.assign(redirectTo);
	}

	function fail(error: OAuthFailure) {
		busy = null;
		switch (error.errorCode) {
			case 'request_not_found':
			case 'already_decided':
				token = null;
				view = { kind: 'expired', client: client() };
				return;
			case 'consent_token_invalid':
				// The token is single use and bound to the account: read the request again.
				if (consent) {
					void load({ kind: 'reload', requestId: consent.requestId }, failureKey(error));
				}
				return;
			case 'workspace_not_eligible':
				if (selection.workspaceId) {
					workspaces = markIneligible(workspaces, selection.workspaceId);
					selection = { ...selection, workspaceId: null, projectIds: [] };
				}
				errorKey = failureKey(error);
				return;
			case 'client_not_active':
				clientAvailable = false;
				errorKey = failureKey(error);
				return;
			case 'too_many_grants':
				view = { kind: 'tooMany', info: parseTooManyGrants(error.details) };
				return;
			default:
				errorKey = failureKey(error);
		}
	}

	async function decide(decision: Decision, kind: Exclude<Busy, 'signout' | null>) {
		if (busy !== null || !consent) return;
		busy = kind;
		errorKey = null;
		const answered = await submitDecision(
			consent.requestId,
			decision,
			decision.decision === 'no_account' ? null : token
		);
		if (!answered.ok) {
			fail(answered.failure);
			return;
		}
		navigate(answered.data.redirectTo, client());
	}

	function allow() {
		if (!consent) return;
		const body = allowBody({
			workspaces,
			scopes: consent.scopes,
			resources: consent.resources,
			selection,
			clientAvailable,
			projectsStatus
		});
		if (body === null) return;
		void decide(body, 'allow');
	}

	async function noAccount() {
		if (busy !== null || view.kind !== 'login') return;
		busy = 'no_account';
		errorKey = null;
		const requestId = view.requestId;
		const name = view.client.displayName;
		const answered = await submitDecision(requestId, { decision: 'no_account' }, null);
		if (!answered.ok) {
			busy = null;
			errorKey =
				answered.failure.errorCode === 'request_not_found' ||
				answered.failure.errorCode === 'already_decided'
					? 'oauth.errors.request_not_found'
					: failureKey(answered.failure);
			return;
		}
		navigate(answered.data.redirectTo, name);
	}

	function signIn() {
		if (view.kind !== 'login') return;
		const target = loginUrlForRequest(view.requestId);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- built from a validated id
		if (target !== null) void goto(target);
	}

	async function signOut() {
		if (busy !== null || !consent) return;
		busy = 'signout';
		const requestId = consent.requestId;
		const ended = await authApi.logout();
		if (apiClient.isAuthResultCurrent(ended)) await authStore.logout();
		busy = null;
		refreshTried = true;
		await load({ kind: 'reload', requestId });
	}

	// Moves focus to a newly shown error so a screen reader announces it and a keyboard user is there.
	$effect(() => {
		if (errorKey && errorBox) errorBox.focus();
	});

	const workspaceName = (id: string): string =>
		workspaces.find((workspace) => workspace.id === id)?.name ?? '';
</script>

<svelte:head>
	<title>{$t('oauth.consent.pageTitle', { values: { client: clientName || 'Sylvode' } })}</title>
	<meta name="referrer" content="no-referrer" />
</svelte:head>

<div class="consent-page" data-testid="consent-page" data-view={view.kind}>
	<header class="brand">
		<span class="wordmark">{$t('oauth.consent.wordmark')}</span>
	</header>

	<main class="column">
		{#if view.kind === 'loading'}
			<div class="stack" data-testid="consent-loading">
				<Skeleton height="28px" width="70%" label={$t('oauth.consent.loading')} />
				<Skeleton height="64px" />
				<Skeleton height="40px" />
				<Skeleton height="120px" />
			</div>
		{:else if view.kind === 'framed'}
			<div class="stack" data-testid="consent-framed">
				<p class="body" role="alert">{$t('oauth.consent.framed')}</p>
			</div>
		{:else if view.kind === 'invalid'}
			<div class="stack" data-testid="consent-error-page">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.consent.invalid.title')}
				</h1>
				<SecurityWarning tone="danger" title={$t('oauth.consent.invalid.title')} role="alert">
					{$t('oauth.consent.invalid.body')}
				</SecurityWarning>
			</div>
		{:else if view.kind === 'expired'}
			<div class="stack" data-testid="consent-expired">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.consent.expired.title')}
				</h1>
				<p class="body">
					{view.client
						? $t('oauth.consent.expired.body', { values: { client: view.client } })
						: $t('oauth.consent.expiredGeneric.body')}
				</p>
			</div>
		{:else if view.kind === 'failed'}
			<div class="stack" data-testid="consent-failed">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.errors.generic')}
				</h1>
				<p class="body" role="alert">{$t(view.messageKey)}</p>
				<div class="actions">
					<Button
						variant="primary"
						data-testid="consent-retry"
						onclick={() => lastInput && void load(lastInput)}
					>
						{$t('oauth.manage.retry')}
					</Button>
				</div>
			</div>
		{:else if view.kind === 'login'}
			<div class="stack" data-testid="consent-login">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.consent.login.title', { values: { client: view.client.displayName } })}
				</h1>
				<section class="origin" data-testid="consent-origin">
					<span class="caption">{$t('oauth.consent.origin.caption')}</span>
					<span class="origin-line">
						{$t('oauth.consent.origin.line', { values: { client: view.client.displayName } })}
					</span>
					<span class="hint">
						{$t('oauth.consent.origin.hint', { values: { host: view.client.redirectHost } })}
					</span>
				</section>
				{#if errorKey}
					<div tabindex="-1" bind:this={errorBox} data-testid="consent-error">
						<TipBar tone="danger">{$t(errorKey)}</TipBar>
					</div>
				{/if}
				<div class="actions start">
					<Button variant="primary" data-testid="consent-login-button" onclick={signIn}>
						{$t('oauth.consent.login.signIn')}
					</Button>
				</div>
				<p class="body muted">{$t('oauth.consent.login.hint')}</p>
				<div class="actions start">
					<Button
						variant="text"
						data-testid="consent-no-account"
						loading={busy === 'no_account'}
						disabled={busy !== null}
						onclick={noAccount}
					>
						{$t('oauth.consent.login.noAccount')}
					</Button>
				</div>
			</div>
		{:else if view.kind === 'done'}
			<div class="stack" data-testid="consent-done">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.consent.done.title', { values: { client: view.client } })}
				</h1>
				<p class="body" role="status">
					{$t('oauth.consent.done.body', { values: { client: view.client } })}
				</p>
			</div>
		{:else if view.kind === 'tooMany' && consent}
			<div class="stack" data-testid="consent-too-many">
				<h1 class="title" tabindex="-1" bind:this={heading}>
					{$t('oauth.consent.tooMany.title', {
						values: {
							max: view.info?.maxActiveGrants ?? consent.maxActiveGrants,
							client: client()
						}
					})}
				</h1>
				<p class="body">{$t('oauth.consent.tooMany.body')}</p>
				{#if view.info}
					<ul class="grants">
						{#each view.info.grants as grant (grant.grantId)}
							<li data-testid="consent-too-many-item">
								{grant.deviceName ?? $t('oauth.consent.tooMany.deviceUnknown')}
								{#if workspaceName(grant.workspaceId)}
									· {workspaceName(grant.workspaceId)}
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				<div class="actions start">
					<!-- A plain link: the Connected applications section of personal settings. -->
					<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- fixed path -->
					<a class="link" href="/settings" data-testid="consent-open-settings">
						{$t('oauth.consent.tooMany.open')}
					</a>
					<Button variant="primary" data-testid="consent-too-many-retry" onclick={allow}>
						{$t('oauth.consent.tooMany.retry')}
					</Button>
				</div>
			</div>
		{:else if view.kind === 'consent' && consent && result}
			{@const c = consent}
			{@const name = c.client.displayName}
			<div class="stack" data-testid="consent-form">
				<h1 class="title" tabindex="-1" bind:this={heading} data-testid="consent-title">
					{$t('oauth.consent.title', { values: { client: name } })}
				</h1>

				<section
					class="origin"
					data-testid="consent-origin"
					aria-label={$t('oauth.consent.origin.caption')}
				>
					<span class="caption">{$t('oauth.consent.origin.caption')}</span>
					<span class="origin-line">
						{c.client.deviceName
							? $t('oauth.consent.origin.lineDevice', {
									values: { client: name, device: c.client.deviceName }
								})
							: $t('oauth.consent.origin.line', { values: { client: name } })}
					</span>
					<span class="hint">
						{$t('oauth.consent.origin.hint', { values: { host: c.client.redirectHost } })}
						{#if c.client.deviceName}
							<br />{$t('oauth.consent.origin.hintDevice')}
						{/if}
					</span>
				</section>

				<div class="account" data-testid="consent-account">
					<span>
						{$t('oauth.consent.account.signedInAs', {
							values: { name: c.account.name, email: c.account.email }
						})}
					</span>
					<Button
						variant="text"
						size="sm"
						data-testid="consent-not-you"
						loading={busy === 'signout'}
						loadingLabel={$t('oauth.consent.account.signingOut')}
						disabled={busy !== null}
						onclick={signOut}
					>
						{$t('oauth.consent.account.notYou')}
					</Button>
				</div>

				{#if errorKey}
					<div tabindex="-1" bind:this={errorBox} data-testid="consent-error">
						<TipBar tone="danger">{$t(errorKey)}</TipBar>
					</div>
				{/if}

				{#if result.noEligibleWorkspace}
					<section class="group" data-testid="consent-no-eligible">
						<h2 class="subtitle">{$t('oauth.consent.noEligible.title')}</h2>
						<p class="body">{$t('oauth.consent.noEligible.body')}</p>
						<div class="actions start">
							<Button
								variant="primary"
								data-testid="consent-return"
								loading={busy === 'deny'}
								disabled={busy !== null}
								onclick={() => decide({ decision: 'deny' }, 'deny')}
							>
								{$t('oauth.consent.noEligible.back', { values: { client: name } })}
							</Button>
						</div>
					</section>
				{:else}
					<fieldset class="group" disabled={busy !== null && busy !== 'allow' ? true : undefined}>
						<legend class="subtitle">{$t('oauth.consent.workspace.legend')}</legend>
						<RadioGroup
							label={$t('oauth.consent.workspace.legend')}
							value={selection.workspaceId ?? undefined}
							onchange={chooseWorkspace}
							name="consent-workspace"
						>
							{#each workspaces as workspace (workspace.id)}
								<div class="choice" class:dim={!workspace.eligible}>
									<Radio
										value={workspace.id}
										disabled={!workspace.eligible || busy !== null}
										data-testid={`consent-workspace-${workspace.id}`}
										aria-describedby={workspace.eligible ? undefined : `ws-reason-${workspace.id}`}
									>
										<span class="choice-text">{workspace.name}</span>
									</Radio>
									{#if !workspace.eligible}
										<span class="reason" id={`ws-reason-${workspace.id}`}>
											{$t('oauth.consent.workspace.ineligible')}
										</span>
									{/if}
								</div>
							{/each}
						</RadioGroup>
					</fieldset>

					<fieldset class="group">
						<legend class="subtitle">{$t('oauth.consent.projects.legend')}</legend>
						<RadioGroup
							label={$t('oauth.consent.projects.legend')}
							value={selection.projectMode ?? undefined}
							onchange={chooseMode}
							name="consent-project-mode"
							disabled={busy !== null}
						>
							<Radio value="all" data-testid="consent-projects-all">
								{$t('oauth.consent.projects.all')}
							</Radio>
							<Radio value="selected" data-testid="consent-projects-selected">
								{$t('oauth.consent.projects.selected')}
							</Radio>
						</RadioGroup>
						{#if result.showProjectPicker}
							<div class="picker" data-testid="consent-project-picker">
								{#if projectsStatus === 'loading'}
									<Skeleton height="32px" label={$t('oauth.consent.projects.loading')} />
								{:else if projectsStatus === 'failed'}
									<p class="body" role="alert">{$t('oauth.consent.projects.loadFailed')}</p>
									<Button
										size="sm"
										data-testid="consent-projects-retry"
										onclick={() =>
											selection.workspaceId && void loadProjects(selection.workspaceId)}
									>
										{$t('oauth.consent.projects.retry')}
									</Button>
								{:else if projects.length === 0}
									<p class="body muted" data-testid="consent-projects-empty">
										{$t('oauth.consent.projects.empty')}
									</p>
								{:else}
									<MultiSelect
										label={$t('oauth.consent.projects.pickerLabel', { values: { client: name } })}
										placeholder={$t('oauth.consent.projects.pickerPlaceholder')}
										options={projectOptions}
										value={[...selection.projectIds]}
										onchange={(ids) => (selection = { ...selection, projectIds: ids })}
										disabled={busy !== null}
										testId="consent-project-select"
									/>
								{/if}
							</div>
						{/if}
					</fieldset>

					<fieldset class="group">
						<legend class="subtitle">{$t('oauth.consent.access.legend')}</legend>
						{#each scopeGroups as entry (entry.group)}
							<div class="scope-group" data-testid={`consent-scope-group-${entry.group}`}>
								<span class="caption">{$t(`oauth.group.${entry.group}`)}</span>
								{#each entry.scopes as scope (scope.name)}
									<div class="scope">
										<Checkbox
											checked={scope.required || selection.optionalScopes.has(scope.name)}
											disabled={scope.required || busy !== null}
											onchange={(event) => toggleOptional(scope.name, event.currentTarget.checked)}
											data-testid={`consent-scope-${scope.name}`}
										>
											<span class="choice-text">
												{scopeLabel(scope.name)}
												{#if scope.required}
													<span class="reason">
														{$t('oauth.consent.access.required', { values: { client: name } })}
													</span>
												{/if}
											</span>
										</Checkbox>
									</div>
								{/each}
							</div>
						{/each}
					</fieldset>

					<SecurityWarning
						tone="warning"
						title={$t('oauth.consent.warning.title', { values: { client: name } })}
						data-testid="consent-warning"
					>
						{$t('oauth.consent.warning.body', { values: { client: name } })}
					</SecurityWarning>

					<div class="actions end">
						<Button
							data-testid="consent-deny"
							loading={busy === 'deny'}
							disabled={busy !== null}
							onclick={() => decide({ decision: 'deny' }, 'deny')}
						>
							{$t('oauth.consent.actions.cancel')}
						</Button>
						<Button
							variant="primary"
							data-testid="consent-allow"
							loading={busy === 'allow'}
							loadingLabel={$t('oauth.consent.actions.connecting')}
							disabled={!result.allowEnabled || busy !== null}
							disabledReason={result.allowBlock && busy === null
								? $t(`oauth.consent.block.${result.allowBlock}`, { values: { client: name } })
								: undefined}
							onclick={allow}
						>
							{$t('oauth.consent.actions.allow', { values: { client: name } })}
						</Button>
					</div>
				{/if}
			</div>
		{/if}
	</main>
</div>

<style>
	.consent-page {
		min-height: 100vh;
		background: var(--rf-bg);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		padding: var(--rf-space-6) var(--rf-space-5) var(--rf-space-9);
	}
	.brand {
		max-width: 960px;
		margin: 0 auto var(--rf-space-7);
	}
	.wordmark {
		font-size: var(--rf-size-subheading);
		font-weight: var(--rf-weight-semibold);
	}
	.column {
		max-width: 560px;
		margin: 0 auto;
	}
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-title);
		line-height: var(--rf-leading-title);
		font-weight: var(--rf-weight-semibold);
		outline: none;
	}
	.subtitle {
		padding: 0;
		margin: 0 0 var(--rf-space-4);
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body-tight);
		font-weight: var(--rf-weight-semibold);
	}
	.body {
		margin: 0;
		font-size: var(--rf-size-body);
		line-height: var(--rf-leading-body);
	}
	.muted,
	.hint,
	.caption,
	.reason {
		color: var(--rf-secondary);
	}
	.origin {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		padding: var(--rf-space-5);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface);
	}
	.caption {
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
	}
	.origin-line {
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-medium);
		overflow-wrap: anywhere;
	}
	.hint,
	.reason {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
	.account {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
		font-size: var(--rf-size-body);
		overflow-wrap: anywhere;
	}
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		min-width: 0;
		margin: 0;
		padding: 0;
		border: 0;
	}
	.group legend {
		float: left;
		width: 100%;
	}
	.group legend + :global(*) {
		clear: both;
	}
	.choice {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
	}
	.choice.dim :global(.ui-radio) {
		color: var(--rf-secondary);
	}
	.choice .reason {
		margin-inline-start: 26px;
		color: var(--rf-text);
	}
	.choice-text {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.picker {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--rf-space-4);
		margin-top: var(--rf-space-4);
	}
	.picker > :global(*) {
		max-width: 100%;
	}
	.scope-group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		margin-bottom: var(--rf-space-4);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: flex-start;
	}
	.actions.end {
		justify-content: flex-end;
	}
	.actions.start {
		justify-content: flex-start;
		align-items: center;
	}
	.grants {
		margin: 0;
		padding-inline-start: var(--rf-space-7);
		font-size: var(--rf-size-body);
	}
	.link {
		color: var(--rf-primary);
		font-size: var(--rf-size-button);
		text-decoration: underline;
	}
	.link:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
</style>
