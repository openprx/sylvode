<script lang="ts">
	/* eslint-disable svelte/no-navigation-without-resolve -- every href here is a full in-app path built by the shell */
	import { onMount, untrack, type Snippet } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { isFlowCanvasRoute } from '#lib/flow/page-layout.js';
	import { projectOptionsStore } from '#lib/stores/project-options.js';
	import { clearFeedback, registerTipSink } from '#lib/surface/feedback.svelte.js';
	import type { StandaloneSurfaceHost } from '#lib/surface/standalone-host.svelte.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import ConfirmHost from '#lib/ui/dialog/ConfirmHost.svelte';
	import '#lib/ui/foundations/visually-hidden.css';
	import { resolvedTheme, setTheme, theme } from '#lib/theme.js';
	import { setLocale } from '#lib/i18n/index.js';
	import { taskResource, type TaskWorkbench } from '#lib/workbench/tabs.js';
	import { normalizeAppLocale, type AccountControls } from './account/account-model';
	import { defaultColumnApis } from './column/column-apis';
	import { ColumnData } from './column/column-data.svelte';
	import BackToListRow from './column/BackToListRow.svelte';
	import { isNarrowWindow, searchHint } from './column/layout';
	import SecondaryColumn from './column/SecondaryColumn.svelte';
	import { scopeChangeTarget } from './column/scope';
	import type { ColumnViewer } from './column/types';
	import { provideShell } from './context';
	import ContentFrame from './content/ContentFrame.svelte';
	import {
		DESTINATIONS,
		PERSONAL_SCOPE,
		SETTINGS_LABEL_KEY,
		SETTINGS_ROOT,
		railHref,
		type FlowAvailability
	} from './destinations';
	import { installShellKeys } from './keyboard';
	import Palette from './palette/Palette.svelte';
	import { defaultPaletteSources } from './palette/palette-api';
	import {
		actionEntries,
		formEntries,
		gotoEntries,
		projectEntries,
		recentEntries,
		type PaletteActionRef,
		type PaletteContext
	} from './palette/palette-sources';
	import NarrowTopBar from './rail/NarrowTopBar.svelte';
	import Rail from './rail/Rail.svelte';
	import { buildRailModel, currentPageShortcut } from './rail/rail-model';
	import { resolveRoute, type Destination } from './route-map';
	import { checkShortcut, type ShellSession } from './session.svelte';
	import { settingsGroups } from './settings-sections';
	import SettingsColumn from './settings/SettingsColumn.svelte';
	import { buildSettingsViewer } from './settings/frame';
	import { ShellState } from './shell-state.svelte';
	import { RailShortcuts } from './shortcuts.svelte';

	interface Props {
		host: StandaloneSurfaceHost;
		workbench: TaskWorkbench;
		session: ShellSession;
		/** The page may render (profile verified, task sessions of the user ready). */
		ready: boolean;
		/** The element that scrolls the active task. */
		contentRoot?: HTMLElement | null;
		onsignout: () => void;
		children: Snippet;
	}

	let {
		host,
		workbench,
		session,
		ready,
		contentRoot = $bindable(null),
		onsignout,
		children
	}: Props = $props();

	// The layout re-creates this component when the signed-in user changes, so everything built
	// here belongs to one user.
	const userId = untrack(() => session.userId);
	const shell = new ShellState({ userId });
	provideShell(shell);
	const shortcuts = new RailShortcuts({ userId });
	const data = new ColumnData(defaultColumnApis);
	const sources = defaultPaletteSources;

	let windowWidth = $state(typeof window === 'undefined' ? 1280 : window.innerWidth);
	let paletteOpen = $state(false);
	let editorOpen = $state(false);
	/** Narrow window: the destination whose list shows full width without navigating. */
	let narrowDestination = $state<Destination | null>(null);

	const narrow = $derived(isNarrowWindow(windowWidth));
	// `page.url` is a read-only view of the URL; the shell's pure functions take a `URL`.
	const url = $derived(new URL(page.url.href));
	const ctx = $derived(resolveRoute(url, page.route.id));
	const flowCanvas = $derived(isFlowCanvasRoute(page.route.id));
	const projects = $derived($projectOptionsStore.items);
	const workspaces = $derived(data.workspaces.items);
	const flow = $derived<FlowAvailability>({
		personal: session.personalFlow,
		workspaces: session.flowWorkspaces
	});
	const lastFor = (key: Parameters<ShellState['lastFor']>[0], scope: string) =>
		shell.lastFor(key, scope);
	const columnDestination = $derived<Destination>(narrowDestination ?? ctx.destination ?? 'home');
	const settingsMode = $derived(ctx.settings !== null && narrowDestination === null);

	const railModel = $derived(
		buildRailModel({
			ctx,
			url,
			scope: shell.scope,
			flow,
			lastFor,
			shortcuts: shortcuts.items,
			availability: (id) => shortcuts.availabilityOf(id),
			unread: session.unread,
			t: $t
		})
	);

	// What "Pin current page to rail" would add; the label comes from what the shell has loaded.
	const currentLabel = $derived.by(() => {
		if (ctx.formId && ctx.projectId)
			return data.formsOf(ctx.projectId).items.find((form) => form.id === ctx.formId)?.name ?? '';
		if (ctx.projectId) return projects.find((p) => p.id === ctx.projectId)?.name ?? '';
		if (ctx.flowObjectId) {
			const tab = $workbench.tabs.find((item) => item.key === $workbench.active);
			return host.title?.title || (tab && !tab.title.startsWith('workbench.') ? tab.title : '');
		}
		return '';
	});
	const currentPage = $derived(
		currentPageShortcut({
			ctx,
			url,
			label: currentLabel,
			projectTypeKey: projects.find((p) => p.id === ctx.projectId)?.type_key ?? null
		})
	);
	$effect(() => {
		if (ctx.formId && ctx.projectId) void data.loadForms(ctx.projectId);
	});

	const viewer = $derived<ColumnViewer>({
		workspaceAdmin: session.isWorkspaceAdmin,
		instanceAdmin: session.isInstanceAdmin,
		flow,
		unread: session.unread
	});
	const settingsViewer = $derived(
		buildSettingsViewer({
			user: session.user,
			workspaceRole: session.workspaceRole,
			personalFlow: session.personalFlow,
			scope: shell.scope,
			workspaces,
			ctx,
			projects
		})
	);

	const account = $derived<AccountControls>({
		user: { name: session.user?.name ?? '', email: session.user?.email ?? '' },
		theme: $theme,
		locale: normalizeAppLocale($locale),
		ontheme: setTheme,
		onlocale: setLocale,
		onsignout
	});

	function confirmRestore(): Promise<boolean> {
		return confirmAction({
			tone: 'neutral',
			title: $t('shell.app.restoreTitle'),
			body: $t('shell.app.restoreBody'),
			confirmLabel: $t('shell.app.restoreConfirm')
		});
	}

	function toggleColumn(destination: Destination) {
		shell.toggleCollapsed(destination, { flowCanvas });
	}
	function chooseDestination(destination: Destination) {
		narrowDestination = destination;
		shell.showList();
	}

	// --- command palette ---

	const paletteContext = $derived.by((): PaletteContext => {
		if (!paletteOpen)
			return {
				goto: [],
				recent: [],
				projects: [],
				forms: [],
				actions: [],
				flowWorkspace: null,
				fallbackWorkspaceId: null
			};
		const scope = shell.scope;
		const lastActive = (href: string) =>
			$workbench.tabs.find((tab) => tab.url === href)?.lastActive ?? 0;
		const forms = projects.flatMap((project) =>
			data.formsOf(project.id).items.map((form) => ({
				id: form.id,
				name: form.name,
				projectId: project.id,
				projectName: project.name,
				workspaceId: project.workspace_id
			}))
		);
		const flowWorkspace =
			scope !== PERSONAL_SCOPE && session.flowWorkspaces[scope] === true
				? { id: scope, name: workspaces.find((w) => w.id === scope)?.name ?? '' }
				: null;
		return {
			goto: gotoEntries({
				t: $t,
				scope,
				flow,
				lastFor,
				settingsGroups: settingsGroups(settingsViewer)
			}),
			recent: recentEntries(shell.recent, $t),
			projects: projectEntries(projects, $t, lastActive),
			forms: formEntries(forms, $t, lastActive),
			actions: actionEntries({
				t: $t,
				scope,
				workspaces,
				theme: $theme,
				locale: normalizeAppLocale($locale),
				pinnable: currentPage?.label ?? null
			}),
			flowWorkspace,
			fallbackWorkspaceId: scope === PERSONAL_SCOPE ? null : scope
		};
	});
	const shortcutHint = $derived(
		searchHint(typeof navigator === 'undefined' ? '' : navigator.platform)
	);

	function openFromPalette(href: string, options: { newTab: boolean }) {
		if (options.newTab) window.open(href, '_blank', 'noopener');
		else void goto(href);
	}

	function runAction(action: PaletteActionRef) {
		switch (action.id) {
			case 'switch-workspace': {
				if (!action.arg) return;
				shell.setScope(action.arg);
				const href = scopeChangeTarget({ ctx, scope: action.arg, flow });
				if (href) void goto(href);
				return;
			}
			case 'theme':
				if (action.arg === 'light' || action.arg === 'dark' || action.arg === 'system')
					setTheme(action.arg);
				return;
			case 'language':
				if (action.arg === 'zh' || action.arg === 'en') setLocale(action.arg);
				return;
			case 'pin-current':
				if (currentPage) shortcuts.add(currentPage);
				return;
			case 'edit-navigation':
				// After the palette has given focus back, so the editor's popover keeps it.
				setTimeout(() => (editorOpen = true), 0);
				return;
			case 'open-settings':
				void goto(railModel.settings.href);
				return;
			case 'sign-out':
				onsignout();
				return;
		}
	}

	// --- lifecycle ---

	$effect(() => host.setTheme($resolvedTheme));
	$effect(() => {
		void data.loadWorkspaces();
	});
	// Rail shortcuts are checked once per session, after the project list is known.
	$effect(() => {
		if ($projectOptionsStore.loaded)
			void shortcuts.validate((item) => checkShortcut(item, projects));
	});

	afterNavigate(() => {
		host.noteNavigation(new URL(page.url.href));
		host.syncTasks();
		clearFeedback();
		const now = new URL(page.url.href);
		shell.observe(now, resolveRoute(now, page.route.id));
		narrowDestination = null;
		shell.showContent();
		const resource = taskResource(page.url.href);
		const tab = $workbench.tabs.find((item) => item.key === resource?.key);
		if (resource && tab) shell.recordVisit(page.url.href, tabTitle(tab.title));
	});
	function tabTitle(title: string): string {
		return title.startsWith('workbench.') ? $t(title) : title;
	}

	onMount(() => {
		const stopTips = registerTipSink((tip) => host.applyTip(tip));
		const stopKeys = installShellKeys({
			globalShortcuts: () => host.globalShortcuts,
			actions: {
				togglePalette: () => (paletteOpen = !paletteOpen),
				openSettings: () => void goto(railModel.settings.href || SETTINGS_ROOT),
				goTo(destination) {
					const route = railHref(destination, shell.scope, lastFor(destination, shell.scope), flow);
					if (route.available) void goto(route.href);
				},
				// The column list handles "P" on its own rows and marks the event as handled.
				addFocusedRow: () => false
			}
		});
		return () => {
			stopTips();
			stopKeys();
		};
	});

	function skipToContent(event: MouseEvent) {
		event.preventDefault();
		document.getElementById('sv-main')?.focus();
	}
	const backLabel = $derived(
		$t(settingsMode ? SETTINGS_LABEL_KEY : DESTINATIONS[columnDestination].labelKey)
	);
	const showList = $derived(!narrow || shell.narrowView === 'list');
	const showContent = $derived(!narrow || shell.narrowView === 'content');
</script>

<svelte:window bind:innerWidth={windowWidth} />

<div class="app" class:narrow data-shell="app">
	<a class="skip ui-sr-only" href="#sv-main" onclick={skipToContent}
		>{$t('shell.app.skipToContent')}</a
	>

	{#if narrow}
		<NarrowTopBar
			model={railModel}
			ondestination={chooseDestination}
			{account}
			{shortcuts}
			{currentPage}
			{confirmRestore}
			bind:editorOpen
		/>
	{:else}
		<div class="rail-slot">
			<Rail
				model={railModel}
				columnExpanded={ctx.destination !== null &&
					!shell.isCollapsed(ctx.destination, { flowCanvas })}
				ontogglecolumn={toggleColumn}
				{account}
				{shortcuts}
				{currentPage}
				{confirmRestore}
				bind:editorOpen
			/>
		</div>
	{/if}

	{#if showList}
		{#if settingsMode}
			<div class="list-slot" class:fill={narrow}>
				<SettingsColumn {ctx} {shell} viewer={settingsViewer} {windowWidth} {data} />
			</div>
		{:else}
			<div class="list-slot" class:fill={narrow}>
				<SecondaryColumn
					destination={columnDestination}
					{ctx}
					{url}
					routeId={page.route.id}
					{shell}
					{viewer}
					{windowWidth}
					{data}
					onsearch={() => (paletteOpen = true)}
					onpin={(shortcut) => shortcuts.add(shortcut)}
				/>
			</div>
		{/if}
	{/if}

	<div class="content-slot" class:hidden={!showContent}>
		{#if narrow}<BackToListRow label={backLabel} onclick={() => shell.showList()} />{/if}
		<ContentFrame {host} {workbench} {userId} {ready} bind:contentRoot>
			{@render children()}
		</ContentFrame>
	</div>
</div>

<Palette
	open={paletteOpen}
	onclose={() => (paletteOpen = false)}
	context={paletteContext}
	{sources}
	onnavigate={openFromPalette}
	onaction={runAction}
	{shortcutHint}
/>
<ConfirmHost />

<style>
	.app {
		display: flex;
		height: 100dvh;
		min-height: 0;
		background: var(--rf-bg);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
	.app.narrow {
		flex-direction: column;
	}
	.rail-slot {
		flex: none;
		height: 100%;
	}
	.list-slot {
		display: flex;
		flex: none;
		min-height: 0;
	}
	.list-slot.fill {
		flex: 1;
	}
	.content-slot {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}
	.content-slot.hidden {
		display: none;
	}
	.skip:focus {
		position: fixed;
		inset-block-start: var(--rf-space-4);
		inset-inline-start: var(--rf-space-4);
		z-index: var(--rf-z-tooltip);
		width: auto;
		height: auto;
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-6);
		clip-path: none;
		border-radius: var(--rf-radius-input);
		background: var(--rf-primary);
		color: var(--rf-on-primary);
	}
</style>
