<script lang="ts">
	import '#lib/i18n/index.js';
	import { setContext } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import { provideSurfaceHostProvider } from '#lib/surface/context.js';
	import { StandaloneSurfaceHost } from '#lib/surface/standalone-host.svelte.js';
	import { workspaceHomeNames } from '#lib/workspace/home.js';
	import { FLOW_TASK_SESSIONS } from '#lib/workbench/flow-sessions.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { isFlowCanvasRoute } from '#lib/flow/page-layout.js';
	import AppShell from '#lib/shell/AppShell.svelte';
	import { PERSONAL_SCOPE, destinationRoot } from '#lib/shell/destinations.js';
	import { pageTitle } from '#lib/shell/page-titles.js';
	import { resolveRoute } from '#lib/shell/route-map.js';
	import { createShellSession } from '#lib/shell/session.svelte.js';
	import { wireWorkbench } from '#lib/shell/workbench-wiring.svelte.js';

	let { children } = $props();

	const session = createShellSession();
	const workbench = new TaskWorkbench();
	let contentRoot = $state<HTMLElement | null>(null);
	const wiring = wireWorkbench({
		workbench,
		session,
		scroll: {
			root: () => contentRoot,
			flowCanvas: () => isFlowCanvasRoute(page.route.id)
		}
	});
	setContext(WORKBENCH_CONTEXT, workbench);
	setContext(FLOW_TASK_SESSIONS, () => wiring.flowTasks);
	setContext(PLUGIN_ADMIN_CONTEXT, () => session.isWorkspaceAdmin);

	// The standalone frame: surfaces declare their title, actions and tip to it, and the shell
	// draws them.
	const host = new StandaloneSurfaceHost({
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- the host passes full in-app paths
		navigate: (url, options) => void goto(url, options),
		// A throwaway copy: the host builds the next address from it.
		currentUrl: () => new URL(page.url.href),
		rootHref: () => {
			const ctx = resolveRoute(new URL(page.url.href), page.route.id);
			const root = destinationRoot(ctx.destination ?? 'home', ctx.workspaceId ?? PERSONAL_SCOPE, {
				personal: session.personalFlow,
				workspaces: session.flowWorkspaces
			});
			return root.available ? root.href : '/workspace';
		},
		translate: (key, values) => $t(key, { values }),
		locale: () => $locale ?? 'en',
		tasks: {
			activeKey: () => workbench.current.active,
			setTitle: (key, title) => workbench.setTitle(key, title),
			markDirty: (key, dirty) => workbench.markDirty(key, dirty)
		}
	});
	provideSurfaceHostProvider(host);

	const title = $derived(
		host.documentTitle ??
			pageTitle(page.url.pathname, (key, options) => $t(key, options), $workspaceHomeNames)
	);
</script>

<svelte:head><title>{title}</title></svelte:head>

{#key session.userId}
	<AppShell
		{host}
		{workbench}
		{session}
		ready={session.ready && wiring.flowTasks?.userId === session.userId}
		bind:contentRoot
		onsignout={() => void session.logout(workbench)}
	>
		{@render children()}
	</AppShell>
{/key}
