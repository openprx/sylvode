<script lang="ts">
	import { isFlowCanvasRoute } from '#lib/flow/page-layout.js';
	import { setContext, getContext } from 'svelte';
	import { FLOW_TASK_SESSIONS, type FlowTaskSessions } from '#lib/workbench/flow-sessions.js';
	import { invalidateAll } from '$app/navigation';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import { routeFailureCopy } from '#lib/flow/route-failure.js';
	import FlowResizablePane from './FlowResizablePane.svelte';
	import FlowNavigator from '#lib/components/flow/FlowNavigator.svelte';
	import { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import {
		FLOW_REPOSITORY_CONTEXT,
		FLOW_SCOPE_CONTEXT,
		FLOW_NAV_VIEW_CONTEXT
	} from '#lib/flow/context-keys.js';

	let {
		data,
		children
	}: {
		data: {
			scopeId?: string;
			workspaceId?: string;
			navigationEnabled?: boolean;
			flowEnabled: boolean;
			error?: string;
			errorCode?: string;
			errorReason?: string;
		};
		children: import('svelte').Snippet;
	} = $props();

	// One repository per Flow route-tree mount, shared by the navigator and whichever
	// `[objectId]/+page.svelte` is currently rendered underneath -- this is what lets navigating
	// between two open Page tabs reuse the same engine-doc lifecycle rules instead of each page
	// route reinventing "how do I open an object".
	const taskSessions = getContext<(() => FlowTaskSessions | null) | undefined>(
		FLOW_TASK_SESSIONS
	)?.();
	const repository = taskSessions?.repository ?? new FlowObjectRepository();
	setContext(FLOW_REPOSITORY_CONTEXT, repository);
	setContext(FLOW_NAV_VIEW_CONTEXT, { refresh: async () => {} });
	setContext(FLOW_SCOPE_CONTEXT, () => {
		if (!data.scopeId) throw new Error('Flow scope unavailable');
		return data.scopeId;
	});

	const failureCopy = $derived(routeFailureCopy(data));
	const selectedObjectId = $derived((page.params as { objectId?: string }).objectId ?? null);

	// Below `lg` the navigator is a drawer over the content, closed by default, so the object,
	// convert and job pages get the full width (390px phones). From `lg` up it is a fixed column.
	let navOpen = $state(false);
	let navToggle = $state<HTMLButtonElement | null>(null);
	$effect(() => {
		void page.url.pathname;
		navOpen = false;
	});

	function onWindowKeydown(event: KeyboardEvent) {
		if (navOpen && event.key === 'Escape') {
			event.preventDefault();
			navOpen = false;
			navToggle?.focus();
		}
	}
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if !data.flowEnabled}
	<div class="flex h-full min-h-[60vh] flex-col items-center justify-center gap-2 p-8 text-center">
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">
			{$t(failureCopy.title)}
		</h1>
		<p class="max-w-md text-sm text-slate-500 dark:text-slate-400">
			{$t(failureCopy.body)}
		</p>
		<button
			type="button"
			class="min-h-11 rounded-md border border-slate-300 px-3 text-sm"
			onclick={() => invalidateAll()}>{$t('flow.personal.retry')}</button
		>
	</div>
{:else}
	<div
		class="flow-workspace flex {isFlowCanvasRoute(page.route.id)
			? 'h-full min-h-0 flex-1'
			: 'h-[calc(100vh-4rem)]'} flex-col overflow-hidden"
	>
		<div
			class="flow-nav-mobile shrink-0 border-b border-slate-200 p-2 dark:border-slate-800"
			hidden={data.navigationEnabled === false}
		>
			<button
				type="button"
				class="inline-flex min-h-[44px] min-w-[44px] items-center gap-2 rounded-md px-3 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-200 dark:hover:bg-slate-800"
				aria-expanded={navOpen}
				aria-controls="flow-navigator-drawer"
				data-testid="flow-nav-toggle"
				bind:this={navToggle}
				onclick={() => (navOpen = !navOpen)}
			>
				<svg
					class="h-5 w-5"
					fill="none"
					stroke="currentColor"
					stroke-width="1.5"
					viewBox="0 0 24 24"
					aria-hidden="true"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						d="M3.75 6.75h16.5M3.75 12h16.5M3.75 17.25h16.5"
					></path>
				</svg>
				<span>{$t(navOpen ? 'flow.nav.hideNavigator' : 'flow.nav.showNavigator')}</span>
			</button>
		</div>
		<div class="relative flex min-h-0 flex-1 overflow-hidden">
			{#if data.navigationEnabled !== false}
				<div
					id="flow-navigator-drawer"
					class="flow-nav-wrapper {navOpen
						? 'flex'
						: 'hidden'} absolute inset-y-0 left-0 z-30 max-w-[85vw] shadow-xl"
					data-testid="flow-nav-drawer"
				>
					<FlowResizablePane side="left" label={$t('flow.layout.resizeNavigation')}>
						<FlowNavigator
							integrationWorkspaceId={data.workspaceId}
							scopeId={data.scopeId!}
							{selectedObjectId}
						/>
					</FlowResizablePane>
				</div>
			{/if}
			<div class="flex min-w-0 flex-1 flex-col overflow-hidden" data-testid="flow-content">
				{@render children()}
			</div>
		</div>
	</div>
{/if}

<style>
	.flow-workspace {
		container-type: inline-size;
		container-name: flowworkspace;
	}
	@container flowworkspace (min-width:900px) {
		.flow-nav-mobile {
			display: none;
		}
		.flow-nav-wrapper {
			position: static;
			z-index: auto;
			display: flex;
			max-width: none;
			box-shadow: none;
		}
	}
</style>
