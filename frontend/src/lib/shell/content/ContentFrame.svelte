<script lang="ts">
	import { setContext, tick, type Snippet } from 'svelte';
	import { t } from 'svelte-i18n';
	import X from '@lucide/svelte/icons/x';
	import { afterNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import TaskTabs from '#lib/components/workbench/TaskTabs.svelte';
	import { isFlowCanvasRoute } from '#lib/flow/page-layout.js';
	import { measureWidth } from '#lib/surface/width.js';
	import type { StandaloneSurfaceHost } from '#lib/surface/standalone-host.svelte.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { createTaskEditSurface, TASK_EDIT_SURFACE } from '#lib/workbench/edit-surface.js';
	import type { TaskWorkbench } from '#lib/workbench/tabs.js';
	import ContentHeader from './ContentHeader.svelte';

	interface Props {
		host: StandaloneSurfaceHost;
		workbench: TaskWorkbench;
		/** The signed-in user; the page below is re-created when it or the path changes. */
		userId: string | null;
		/** False while the profile check runs: the page is replaced by a loading line. */
		ready: boolean;
		/** The element that scrolls the active task; the layout restores scroll positions on it. */
		contentRoot?: HTMLElement | null;
		children: Snippet;
	}

	let { host, workbench, userId, ready, contentRoot = $bindable(null), children }: Props = $props();

	let editOverlay: HTMLElement | null = null;
	let viewBackground: HTMLElement | null = null;
	let scopedEditorCount = $state(0);
	setContext(
		TASK_EDIT_SURFACE,
		createTaskEditSurface(
			() => editOverlay,
			() => viewBackground,
			(count) => (scopedEditorCount = count)
		)
	);

	const flowCanvas = $derived(isFlowCanvasRoute(page.route.id));
	const tip = $derived(host.tip);
	const showHeader = $derived(host.hasSurface && host.header !== 'none');
	const tabs = $derived($workbench);

	// After a client navigation to a page with a header, the title takes focus once.
	let header = $state<ReturnType<typeof ContentHeader> | null>(null);
	let focusPending = false;
	afterNavigate((navigation) => {
		focusPending =
			navigation.type !== 'enter' && navigation.from?.url.pathname !== navigation.to?.url.pathname;
	});
	$effect(() => {
		if (!focusPending || !showHeader || host.loading || !header) return;
		focusPending = false;
		void tick().then(() => header?.focusTitle());
	});
</script>

<main id="sv-main" class="frame" data-shell="content" tabindex="-1">
	<TaskTabs {workbench} />
	{#each tabs.tabs.filter((tab) => tab.key !== tabs.active) as tab (tab.key)}<div
			role="tabpanel"
			id={`task-panel-${encodeURIComponent(tab.key)}`}
			aria-labelledby={`task-tab-${encodeURIComponent(tab.key)}`}
			hidden
		></div>{/each}

	{#if showHeader}
		<div class="head" data-shell="header"><ContentHeader bind:this={header} {host} /></div>
	{/if}
	{#if tip}
		<div class="tip" data-shell="tip">
			<div class="tip-bar">
				<TipBar
					tone={tip.tone}
					action={tip.action
						? { label: tip.action.label, run: () => void tip.action?.run() }
						: undefined}
				>
					{tip.message}
				</TipBar>
			</div>
			{#if tip.dismissible ?? tip.tone === 'success'}
				<IconButton
					icon={X}
					label={$t('shell.content.dismissTip')}
					onclick={() => host.applyTip(null)}
				/>
			{/if}
		</div>
	{/if}

	<div class="area">
		<div
			bind:this={contentRoot}
			class="scroll"
			class:canvas={flowCanvas}
			data-testid="task-tabpanel"
			data-tab-key={tabs.active ?? ''}
			role={tabs.active ? 'tabpanel' : undefined}
			id={tabs.active ? `task-panel-${encodeURIComponent(tabs.active)}` : undefined}
			aria-labelledby={tabs.active ? `task-tab-${encodeURIComponent(tabs.active)}` : undefined}
		>
			<div
				class="surface"
				class:contained={host.hasSurface}
				data-shell="surface"
				data-surface-width={host.widthClass}
				use:measureWidth={(px) => host.setWidth(px)}
			>
				<div
					bind:this={viewBackground}
					class="background"
					inert={scopedEditorCount > 0}
					aria-hidden={scopedEditorCount > 0 ? 'true' : undefined}
				>
					{#if ready && userId}{#key `${userId}:${page.url.pathname}`}{@render children()}{/key}{:else}<p
							role="status"
						>
							{$t('common.loading')}
						</p>{/if}
				</div>
			</div>
		</div>
		<div bind:this={editOverlay} class="overlay" data-testid="task-edit-surface"></div>
	</div>
</main>

<style>
	.frame {
		--content-pad: var(--rf-space-9);
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
		min-height: 0;
		overflow: hidden;
		background: var(--rf-bg);
		color: var(--rf-text);
	}
	.frame:focus-visible {
		outline: none;
	}
	@media (max-width: 1023px) {
		.frame {
			--content-pad: var(--rf-space-8);
		}
	}
	@media (max-width: 800px) {
		.frame {
			--content-pad: var(--rf-space-6);
		}
	}
	.head {
		flex: none;
		padding-inline: var(--content-pad);
	}
	.tip {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		flex: none;
		padding: var(--rf-space-4) var(--content-pad) 0;
	}
	.tip-bar {
		flex: 1;
		min-width: 0;
	}
	.area {
		position: relative;
		flex: 1;
		min-height: 0;
	}
	.scroll {
		box-sizing: border-box;
		height: 100%;
		overflow: auto;
		padding: var(--content-pad);
	}
	.scroll.canvas {
		display: flex;
		flex-direction: column;
		min-height: 0;
		overflow: hidden;
		padding: 0;
	}
	.surface {
		min-width: 0;
	}
	/* Only a surface page measures itself with container queries; legacy pages keep their layout
	   (a size container would also re-anchor their fixed-position descendants). */
	.surface.contained {
		container-type: inline-size;
		container-name: surface;
	}
	.canvas > .surface {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
	}
	.background {
		min-width: 0;
	}
	.canvas .background {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-height: 0;
	}
	.overlay {
		pointer-events: none;
		position: absolute;
		inset: 0;
		z-index: var(--rf-z-panel);
	}
</style>
