<script lang="ts">
	import { untrack } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { projectDiscovery } from '#lib/plugins/contributions.js';
	import { projectOptionsStore as appOptionsStore } from '#lib/stores/project-options.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { PERSONAL_SCOPE, type ScopeKey } from '../destinations';
	import type { RouteContext } from '../route-map';
	import type { Shortcut } from '../shortcuts.svelte';
	import type { ShellState } from '../shell-state.svelte';
	import { projectTree } from '../../project/nav';
	import { appsCountOf } from './apps-count';
	import type { ColumnData } from './column-data.svelte';
	import {
		formsGroupToExpand,
		projectToExpand,
		projectsOfWorkspace,
		shouldRefreshProjects,
		workspaceChoiceRows,
		type Translate
	} from './lists';
	import NavList from './NavList.svelte';
	import {
		emptyIdSet,
		flattenTree,
		selectedRowId,
		toggledIds,
		unionIds,
		type NavRow
	} from './rows';
	import SkeletonRows from './SkeletonRows.svelte';
	import type { ColumnViewer, ProjectOptionsSource, RouteUrl } from './types';

	interface Props {
		shell: ShellState;
		scope: ScopeKey;
		url: RouteUrl;
		ctx: RouteContext;
		viewer: ColumnViewer;
		data: ColumnData;
		onfollow?: (row?: NavRow) => void;
		/** Project option source; the app's store by default. */
		optionsStore?: ProjectOptionsSource;
		onpin?: (shortcut: Shortcut) => void;
	}
	let {
		shell,
		scope,
		url,
		ctx,
		viewer,
		data,
		onfollow,
		onpin,
		optionsStore = appOptionsStore
	}: Props = $props();

	const tr: Translate = (key, values) => $t(key, values ? { values } : undefined);
	const personal = $derived(scope === PERSONAL_SCOPE);

	// --- workspace scope ---
	const options = $derived($optionsStore);
	const projects = $derived(projectsOfWorkspace(options.items, scope, $locale ?? undefined));

	/** Forms groups that are open; memory only (the project expansion is what survives reload). */
	let openForms = $state.raw<ReadonlySet<string>>(emptyIdSet());
	const expanded = $derived(unionIds(shell.expandedProjects(scope), openForms));

	const branches = $derived(
		projects.map((project) => {
			const forms = data.formsOf(project.id);
			const tree = projectTree({
				workspaceId: scope,
				project: { id: project.id, name: project.name, type_key: project.type_key },
				isWorkspaceAdmin: viewer.workspaceAdmin,
				appsCount: data.appsCountOf(project.id),
				forms: forms.status === 'ready' ? forms.items : null,
				formsTotal: forms.status === 'ready' ? forms.total : null,
				translate: (key) => tr(key)
			});
			const rows = flattenTree(tree, expanded, { workspaceId: scope });
			return {
				project,
				forms,
				rows,
				head: rows[0],
				children: rows.slice(1),
				open: rows[0].expanded === true,
				formsOpen: openForms.has(`forms:${project.id}`),
				current: ctx.workspaceId === scope && ctx.projectId === project.id
			};
		})
	);
	const allRows = $derived(branches.flatMap((b) => b.rows));

	// --- personal scope ---
	const workspaceRows = $derived(workspaceChoiceRows(data.workspaces.items));

	const selectedId = $derived(selectedRowId(personal ? workspaceRows : allRows, url));

	function toggle(id: string, open: boolean) {
		const project = projects.find((p) => p.id === id);
		if (project) {
			shell.setProjectExpanded(scope, id, open);
			return;
		}
		openForms = toggledIds(openForms, id, open);
	}

	function pin(row: NavRow) {
		if (row.pin) onpin?.(row.pin);
	}

	function follow(row?: NavRow) {
		const workspaceId = row?.attrs?.['data-workspace-id'];
		if (personal && workspaceId) shell.setScope(workspaceId);
		onfollow?.(row);
	}

	// Opening the route's project (and its Forms group when a form is open) happens once per
	// navigation, so collapsing it by hand sticks.
	$effect(() => {
		const key = `${scope}|${url.pathname}${url.search}`;
		void key;
		untrack(() => {
			const projectId = projectToExpand(ctx, scope, (id) => shell.isProjectExpanded(scope, id));
			if (projectId) shell.setProjectExpanded(scope, projectId, true);
			const formsProject = formsGroupToExpand(ctx);
			if (formsProject && ctx.workspaceId === scope && !openForms.has(`forms:${formsProject}`))
				openForms = toggledIds(openForms, `forms:${formsProject}`, true);
		});
	});

	// Forms of each expanded project load on first expansion; the forms page without a selected
	// form reloads them (a form may have been added or removed).
	$effect(() => {
		for (const branch of branches) if (branch.open) void data.loadForms(branch.project.id);
	});
	$effect(() => {
		const key = `${url.pathname}${url.search}`;
		void key;
		untrack(() => {
			if (
				ctx.projectId &&
				ctx.workspaceId === scope &&
				!ctx.formId &&
				url.pathname.endsWith('/forms')
			)
				void data.loadForms(ctx.projectId, true);
		});
	});

	// The Apps row needs the project's app count; discovery is retained while the project is open.
	// A string key, so the effect re-runs only when the set of open projects changes: an array
	// would be new on every update of the counts and make retain and release loop.
	const openProjectKey = $derived(
		branches
			.filter((b) => b.open)
			.map((b) => b.project.id)
			.join('\n')
	);
	$effect(() => {
		const stops = (openProjectKey ? openProjectKey.split('\n') : []).map((id) => {
			const discovery = projectDiscovery(id);
			// Untracked: the first call runs inside this effect, and reading the counts there would
			// make the effect depend on them and restart discovery whenever one changes.
			const stop = discovery.subscribe((state) =>
				untrack(() => data.setAppsCount(id, appsCountOf(state)))
			);
			const release = discovery.retain();
			return () => {
				stop();
				release();
			};
		});
		return () => stops.forEach((stop) => stop());
	});

	// The list refreshes when the user opens the projects page and when the window regains focus,
	// at most once a minute.
	let lastRefresh: number | null = null;
	function refresh() {
		lastRefresh = Date.now();
		void optionsStore.ensureLoaded(true);
	}
	$effect(() => {
		const onProjectsPage =
			!personal && ctx.workspaceId === scope && url.pathname === `/workspace/${scope}/projects`;
		if (onProjectsPage) untrack(refresh);
	});
	function onWindowFocus() {
		if (!personal && shouldRefreshProjects(Date.now(), lastRefresh)) refresh();
	}
</script>

<svelte:window onfocus={onWindowFocus} />

{#if personal}
	<p class="caption">{$t('shell.column.projects.personalCaption')}</p>
	{#if data.workspaces.status === 'idle' || data.workspaces.status === 'loading'}
		<SkeletonRows thing={$t('shell.column.things.workspaces')} />
	{:else if data.workspaces.status === 'error' || data.workspaces.status === 'forbidden'}
		<StateBlock
			kind={data.workspaces.status === 'forbidden' ? 'forbidden' : 'loadFailed'}
			thing={$t('shell.column.things.workspaces')}
			action={{ run: () => void data.loadWorkspaces(true) }}
		/>
	{:else if workspaceRows.length === 0}
		<StateBlock kind="empty" thing={$t('shell.column.things.workspaces')} />
	{:else}
		<NavList rows={workspaceRows} {selectedId} onfollow={follow} />
	{/if}
{:else if !options.loaded && options.loading}
	<SkeletonRows thing={$t('shell.column.things.projects')} />
{:else if options.error && projects.length === 0}
	<StateBlock
		kind="loadFailed"
		thing={$t('shell.column.things.projects')}
		action={{ run: () => void optionsStore.ensureLoaded(true) }}
	/>
{:else if options.loaded && projects.length === 0}
	<StateBlock kind="empty" thing={$t('shell.column.things.projects')} />
{:else}
	<div class="branches">
		{#each branches as branch (branch.project.id)}
			<div class="branch" data-project-branch={branch.project.id}>
				<NavList
					rows={[branch.head]}
					{selectedId}
					ontoggle={toggle}
					onpin={pin}
					onfollow={follow}
				/>
				{#if branch.open}
					{#if branch.current}
						<nav aria-label={$t('shell.column.projects.current')}>
							<NavList
								rows={branch.children}
								{selectedId}
								ontoggle={toggle}
								onpin={pin}
								onfollow={follow}
							/>
						</nav>
					{:else}
						<NavList
							rows={branch.children}
							{selectedId}
							ontoggle={toggle}
							onpin={pin}
							onfollow={follow}
						/>
					{/if}
					{#if branch.formsOpen}
						{#if branch.forms.status === 'loading'}
							<SkeletonRows thing={$t('shell.column.things.forms')} count={2} />
						{:else if branch.forms.status === 'error' || branch.forms.status === 'forbidden'}
							<StateBlock
								kind={branch.forms.status === 'forbidden' ? 'forbidden' : 'loadFailed'}
								thing={$t('shell.column.things.forms')}
								action={{ run: () => void data.loadForms(branch.project.id, true) }}
							/>
						{/if}
					{/if}
				{/if}
			</div>
		{/each}
	</div>
{/if}

<style>
	.branches {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.branch {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
	}
	.caption {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-4) var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-medium);
	}
</style>
