<script lang="ts">
	import { setContext } from 'svelte';
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import FlowNavigator from '$lib/components/flow/FlowNavigator.svelte';
	import { FlowObjectRepository } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';

	let { data, children } = $props();

	// One repository per Flow route-tree mount, shared by the navigator and whichever
	// `[objectId]/+page.svelte` is currently rendered underneath -- this is what lets navigating
	// between two open Page tabs reuse the same engine-doc lifecycle rules instead of each page
	// route reinventing "how do I open an object".
	const repository = new FlowObjectRepository();
	setContext(FLOW_REPOSITORY_CONTEXT, repository);

	const selectedObjectId = $derived(($page.params as { objectId?: string }).objectId ?? null);

	// Below `lg` the navigator is a drawer over the content, closed by default, so the object,
	// convert and job pages get the full width (390px phones). From `lg` up it is a fixed column.
	let navOpen = $state(false);
	$effect(() => {
		void $page.url.pathname;
		navOpen = false;
	});

	function onWindowKeydown(event: KeyboardEvent) {
		if (navOpen && event.key === 'Escape') navOpen = false;
	}
</script>

<svelte:window onkeydown={onWindowKeydown} />

{#if !data.flowEnabled}
	<div class="flex h-full min-h-[60vh] flex-col items-center justify-center gap-2 p-8 text-center">
		<h1 class="text-lg font-semibold text-slate-900 dark:text-slate-100">{$t('flow.route.disabledTitle')}</h1>
		<p class="max-w-md text-sm text-slate-500 dark:text-slate-400">{$t('flow.route.disabledBody')}</p>
	</div>
{:else}
	<div class="flex h-[calc(100vh-4rem)] flex-col overflow-hidden lg:flex-row">
		<div class="shrink-0 border-b border-slate-200 p-2 lg:hidden dark:border-slate-800">
			<button
				type="button"
				class="inline-flex min-h-[44px] min-w-[44px] items-center gap-2 rounded-md px-3 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-200 dark:hover:bg-slate-800"
				aria-expanded={navOpen}
				aria-controls="flow-navigator-drawer"
				data-testid="flow-nav-toggle"
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
					/>
				</svg>
				<span>{$t(navOpen ? 'flow.nav.hideNavigator' : 'flow.nav.showNavigator')}</span>
			</button>
		</div>
		<div class="relative flex min-h-0 flex-1 overflow-hidden">
			<div
				id="flow-navigator-drawer"
				class="{navOpen
					? 'flex'
					: 'hidden'} absolute inset-y-0 left-0 z-30 max-w-[85vw] shadow-xl lg:static lg:z-auto lg:flex lg:max-w-none lg:shadow-none"
				data-testid="flow-nav-drawer"
			>
				<FlowNavigator workspaceId={data.workspaceId} {selectedObjectId} />
			</div>
			<div class="flex min-w-0 flex-1 flex-col overflow-hidden" data-testid="flow-content">
				{@render children()}
			</div>
		</div>
	</div>
{/if}
