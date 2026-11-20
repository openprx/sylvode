import { onDestroy, onMount } from 'svelte';
import { get } from 'svelte/store';
import { t } from 'svelte-i18n';
import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { page } from '$app/state';
import { apiClient } from '#lib/api/client.js';
import { authApi, type User } from '#lib/api/auth.js';
import { flowApi } from '#lib/api/flow.js';
import { formsApi } from '#lib/api/forms.js';
import { notificationsApi } from '#lib/api/notifications.js';
import { workspacesApi } from '#lib/api/workspaces.js';
import { FLOW_PROCESS_READER_COMPATIBILITY } from '#lib/flow/product-mode.js';
import { authStore, verifySession } from '#lib/stores/auth.js';
import { flowFeatureStore } from '#lib/stores/flow-feature.js';
import { projectOptionsStore, type ProjectOption } from '#lib/stores/project-options.js';
import { confirmAction } from '#lib/ui/dialog/confirm.js';
import { isAdminUser } from '#lib/utils/auth.js';
import { type WorkspaceRole } from '#lib/workspace/home.js';
import { WorkspaceRoleTracker, showWorkspaceAdminLinks } from '#lib/workspace/role.js';
import type { AvailabilityCheck, Shortcut } from './shortcuts.svelte';

const UNREAD_POLL_MS = 30000;

/** What the sign-out guard needs from the task workbench. */
export interface SignOutGuard {
	capture(): void;
	hasDirty(): boolean;
}

/**
 * What the app shell knows about the signed-in user: the verified profile, the unread count, the
 * personal Flow capability, the member role in the workspace on screen and the Flow flag of each
 * workspace. Create it with `createShellSession()` during the layout's initialisation.
 */
export class ShellSession {
	user = $state.raw<User | null>(null);
	/** True until the profile check, the unread count and the project list of the user have landed. */
	loadingProfile = $state(true);
	unread = $state(0);
	personalFlow = $state(false);
	workspaceRole = $state<WorkspaceRole | null>(null);
	flowWorkspaces = $state.raw<Record<string, boolean>>({});
	/** Workspace id of the current URL, or null. */
	workspaceId = $derived(/^\/workspace\/([^/]+)/.exec(page.url.pathname)?.[1] ?? null);

	readonly userId = $derived(this.user?.id ?? null);
	readonly isInstanceAdmin = $derived(isAdminUser(this.user));
	/** Member role only, as on the server; the instance-admin flag does not grant workspace links. */
	readonly isWorkspaceAdmin = $derived(
		showWorkspaceAdminLinks({
			workspaceRole: this.workspaceRole,
			instanceAdmin: this.isInstanceAdmin
		})
	);
	/** Signed in and verified: the content may render. */
	readonly ready = $derived(!this.loadingProfile && this.userId !== null);

	/** Sign out. Unsaved task tabs ask for confirmation through the shared dialog first. */
	async logout(guard: SignOutGuard): Promise<void> {
		guard.capture();
		if (guard.hasDirty()) {
			const translate = get(t);
			const confirmed = await confirmAction({
				tone: 'destructive',
				title: translate('shell.app.signOutTitle'),
				body: translate('shell.app.signOutBody'),
				confirmLabel: translate('shell.app.signOutConfirm')
			});
			if (!confirmed) return;
		}
		const result = await authApi.logout();
		if (!apiClient.isAuthResultCurrent(result)) return;
		const cleared = authStore.logout();
		const epoch = apiClient.getAuthSessionEpoch();
		await cleared;
		if (apiClient.isAuthSessionEpoch(epoch)) goto(resolve('/(auth)/auth/login'));
	}
}

/**
 * Answers one rail shortcut's availability check: a project is present in the project list, a
 * form and a Flow page are readable. Only 403 and 404 (or a missing project) mean "gone".
 */
export async function checkShortcut(
	shortcut: Shortcut,
	projects: readonly ProjectOption[]
): Promise<AvailabilityCheck> {
	if (shortcut.kind === 'project')
		return projects.some((p) => p.id === shortcut.projectId) ? 'available' : 'gone';
	const result =
		shortcut.kind === 'form' && shortcut.formId
			? await formsApi.get(shortcut.formId)
			: shortcut.kind === 'flow-page' && shortcut.objectId
				? await flowApi.getObject(shortcut.objectId)
				: null;
	if (!result) return 'available';
	if (result.code === 0) return 'available';
	return result.code === 403 || result.code === 404 ? 'gone' : 'error';
}

/** Creates the session and starts everything that has to run for the life of the layout. */
export function createShellSession(): ShellSession {
	const session = new ShellSession();

	const stops = [
		authStore.subscribe((value) => (session.user = value.user ?? null)),
		flowFeatureStore.subscribe((value) => (session.flowWorkspaces = value))
	];

	let mounted = $state(false);
	let actor = $state<string | null | undefined>(undefined);
	let run = 0;

	// One Flow flag read per workspace change; any non-success result reads as disabled.
	$effect(() => {
		const workspaceId = session.workspaceId;
		if (workspaceId) void flowFeatureStore.refresh(workspaceId);
	});

	// Re-read the member role whenever the workspace (or user) changes; null until it lands.
	const roleTracker = new WorkspaceRoleTracker(
		(workspaceId) => workspacesApi.getMembers(workspaceId),
		(role) => {
			session.workspaceRole = role;
		}
	);
	$effect(() => {
		void roleTracker.select(session.workspaceId, session.userId);
	});

	$effect(() => {
		projectOptionsStore.selectUser(session.userId);
		if (session.userId && !session.loadingProfile) void projectOptionsStore.ensureLoaded();
	});

	async function fetchUnread() {
		const response = await notificationsApi.getUnreadCount();
		if (response.code === 0 && response.data) session.unread = response.data.count;
	}

	async function loadProfile(): Promise<void> {
		const mine = ++run;
		session.loadingProfile = true;
		// A failed session check ends the session but keeps this user's local Flow data; only
		// the user's own sign-out deletes it.
		let verifiedEpoch = apiClient.getAuthSessionEpoch();
		const verification = await verifySession(
			() => authApi.me(),
			(epoch) => (verifiedEpoch = epoch)
		);
		if (
			mine !== run ||
			verification === 'superseded' ||
			!apiClient.isAuthSessionEpoch(verifiedEpoch)
		)
			return;
		if (verification === 'ended') {
			goto(resolve('/(auth)/auth/login'));
			return;
		}

		projectOptionsStore.selectUser(get(authStore).user?.id ?? null);
		await Promise.all([
			fetchUnread(),
			projectOptionsStore.ensureLoaded(),
			flowApi.getCapabilities().then((result) => {
				if (mine !== run || !apiClient.isAuthSessionEpoch(verifiedEpoch)) return;
				session.personalFlow =
					result.code === 0 &&
					result.data?.api_version === 2 &&
					result.data.minimum_reader_version <= FLOW_PROCESS_READER_COMPATIBILITY;
			})
		]);

		if (mine !== run || !apiClient.isAuthSessionEpoch(verifiedEpoch)) return;
		session.loadingProfile = false;
	}

	$effect(() => {
		const next = session.userId;
		if (mounted && actor !== next) {
			actor = next;
			void loadProfile();
		}
	});

	onMount(() => {
		mounted = true;
		const timer = setInterval(() => void fetchUnread(), UNREAD_POLL_MS);
		return () => clearInterval(timer);
	});
	onDestroy(() => {
		mounted = false;
		run++;
		for (const stop of stops) stop();
	});

	return session;
}
