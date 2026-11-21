<script lang="ts">
	/**
	 * Admin test surface: hosts one Sylvode surface in four frames, the way another application
	 * would. It plays the host: it answers `sv:ready` with `sv:init`, shows what the frames report
	 * (title, actions, tip, close, open) and can change theme, density and width. Every message
	 * received from a frame is also kept in `window.__svMessages` for the automated checks.
	 */
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import Button from '#lib/ui/button/Button.svelte';
	import { embedOrigins } from '#lib/surface/embed-config.js';
	import {
		decodeFrameMessage,
		envelope,
		type ActionDescriptor,
		type FrameMessage,
		type HostMessage,
		type TargetDescriptor,
		type TipDescriptor
	} from '#lib/surface/embed-protocol.js';
	import { recordSurface } from '#lib/surface/registry.js';
	import { widthClass } from '#lib/surface/width.js';
	import type { SurfaceDensity, SurfaceTheme } from '#lib/surface/context.js';

	interface LogEntry {
		frameId: string;
		type: FrameMessage['type'];
		payload: FrameMessage['payload'];
		at: number;
	}
	interface FrameState {
		id: string;
		/** Fixed width in px; null fills the harness. */
		fixed: number | null;
		measured: number;
		element: HTMLIFrameElement | null;
		title: string;
		identifier: string;
		context: string;
		origin: string;
		primary: ActionDescriptor | null;
		secondary: ActionDescriptor[];
		tip: TipDescriptor | null;
		dirty: boolean;
		closeRequests: number;
		opened: TargetDescriptor[];
	}

	const origin = embedOrigins()[0] ?? '';
	const params = $derived(
		page.url.searchParams.get('surface') === 'record'
			? recordSurface.fromQuery(new URLSearchParams(page.url.search))
			: null
	);

	let theme = $state<SurfaceTheme>('light');
	let density = $state<SurfaceDensity>('comfortable');
	let log = $state.raw<LogEntry[]>([]);
	let frames = $state<FrameState[]>(
		[340, 480, 600, null].map((fixed) => ({
			id: fixed === null ? 'harness-full' : `harness-${fixed}`,
			fixed,
			measured: fixed ?? 0,
			element: null,
			title: '',
			identifier: '',
			context: '',
			origin: '',
			primary: null,
			secondary: [],
			tip: null,
			dirty: false,
			closeRequests: 0,
			opened: []
		}))
	);

	/** The frame's inner width: bound to the iframe, so it follows every resize. */
	function widthOf(frame: FrameState): number {
		return frame.measured;
	}

	function post(frame: FrameState, message: HostMessage): void {
		frame.element?.contentWindow?.postMessage(message, origin);
	}

	function init(frame: FrameState): void {
		post(
			frame,
			envelope(frame.id, 'sv:init', {
				slot: 'panel' as const,
				width: widthOf(frame),
				theme,
				density
			})
		);
	}

	function setAppearance(next: { theme?: SurfaceTheme; density?: SurfaceDensity }): void {
		theme = next.theme ?? theme;
		density = next.density ?? density;
		for (const frame of frames) post(frame, envelope(frame.id, 'sv:theme', { theme, density }));
	}

	function setWidth(frame: FrameState, value: string): void {
		const px = Math.round(Number(value));
		if (!Number.isFinite(px) || px < 200 || px > 2000) return;
		frame.fixed = px;
		frame.measured = px;
		post(frame, envelope(frame.id, 'sv:resize', { width: px }));
	}

	function runAction(frame: FrameState, id: string): void {
		post(frame, envelope(frame.id, 'sv:action', { id }));
	}

	function runTipAction(frame: FrameState): void {
		if (frame.tip) post(frame, envelope(frame.id, 'sv:tip-action', { id: frame.tip.id }));
	}

	function onMessage(event: MessageEvent): void {
		// Same-origin phase: only this origin, and only a window this page created.
		if (event.origin !== origin) return;
		const frame = frames.find((item) => item.element?.contentWindow === event.source);
		if (!frame) return;
		const message = decodeFrameMessage(event.data);
		if (!message || message.frameId !== frame.id) return;
		log = [
			...log,
			{ frameId: frame.id, type: message.type, payload: message.payload, at: Date.now() }
		];
		exposeLog();
		switch (message.type) {
			case 'sv:ready':
				init(frame);
				break;
			case 'sv:title':
				frame.title = message.payload.title;
				frame.identifier = message.payload.identifier ?? '';
				frame.context = message.payload.context ?? '';
				frame.origin = message.payload.origin;
				break;
			case 'sv:actions':
				frame.primary = message.payload.primary ?? null;
				frame.secondary = message.payload.secondary;
				break;
			case 'sv:tip':
				frame.tip = message.payload.tip;
				break;
			case 'sv:dirty':
				frame.dirty = message.payload.dirty;
				break;
			case 'sv:close':
				frame.closeRequests += 1;
				break;
			case 'sv:open':
				frame.opened = [...frame.opened, message.payload.target];
				break;
		}
	}

	function exposeLog(): void {
		(window as Window & { __svMessages?: LogEntry[] }).__svMessages = log;
	}

	onMount(() => {
		exposeLog();
		window.addEventListener('message', onMessage);
		return () => window.removeEventListener('message', onMessage);
	});
</script>

<svelte:head><title>{$t('surface.embed.harness.title')}</title></svelte:head>

<main class="harness" data-testid="embed-harness">
	<h1>{$t('surface.embed.harness.title')}</h1>
	<p class="intro">{$t('surface.embed.harness.intro')}</p>

	{#if !params}
		<p data-testid="harness-missing">{$t('surface.embed.harness.missing')}</p>
	{:else}
		<div class="controls">
			<span>{$t('surface.embed.harness.theme')}</span>
			<Button
				size="sm"
				data-testid="harness-theme-light"
				onclick={() => setAppearance({ theme: 'light' })}
				>{$t('surface.embed.harness.themeLight')}</Button
			>
			<Button
				size="sm"
				data-testid="harness-theme-dark"
				onclick={() => setAppearance({ theme: 'dark' })}
				>{$t('surface.embed.harness.themeDark')}</Button
			>
			<span>{$t('surface.embed.harness.density')}</span>
			<Button
				size="sm"
				data-testid="harness-density-comfortable"
				onclick={() => setAppearance({ density: 'comfortable' })}
				>{$t('surface.embed.harness.densityComfortable')}</Button
			>
			<Button
				size="sm"
				data-testid="harness-density-compact"
				onclick={() => setAppearance({ density: 'compact' })}
				>{$t('surface.embed.harness.densityCompact')}</Button
			>
		</div>

		<div class="frames">
			{#each frames as frame (frame.id)}
				<section class="host" data-testid="harness-host" data-frame-id={frame.id}>
					<h2>
						{frame.fixed === null
							? $t('surface.embed.harness.frameFull')
							: $t('surface.embed.harness.frameWidth', { values: { width: frame.fixed } })}
					</h2>
					<label class="width">
						<span>{$t('surface.embed.harness.widthLabel')}</span>
						<input
							type="number"
							min="200"
							max="2000"
							value={frame.fixed ?? ''}
							data-testid="harness-width"
							onchange={(event) => setWidth(frame, event.currentTarget.value)}
						/>
					</label>
					<dl class="chrome">
						<dt>{$t('surface.embed.harness.hostTitle')}</dt>
						<dd data-testid="host-title">
							{frame.identifier ? `${frame.identifier} ` : ''}{frame.title}
						</dd>
						<dt>{$t('surface.embed.harness.hostOrigin')}</dt>
						<dd data-testid="host-origin">{frame.origin}</dd>
						<dt>{$t('surface.embed.harness.hostContext')}</dt>
						<dd data-testid="host-context">{frame.context}</dd>
						<dt>{$t('surface.embed.harness.widthClass')}</dt>
						<dd data-testid="host-width-class">{widthClass(widthOf(frame))}</dd>
						<dt>{$t('surface.embed.harness.hostActions')}</dt>
						<dd class="actions">
							{#each frame.primary ? [frame.primary, ...frame.secondary] : frame.secondary as action (action.id)}
								<Button
									size="sm"
									variant={action.kind === 'primary' ? 'primary' : 'secondary'}
									disabled={!!action.disabledReason}
									disabledReason={action.disabledReason}
									data-testid="host-action"
									data-action-id={action.id}
									onclick={() => runAction(frame, action.id)}>{action.label}</Button
								>
							{:else}
								{$t('surface.embed.harness.hostNoActions')}
							{/each}
						</dd>
						{#if frame.tip}
							<dt>{$t('surface.embed.harness.hostTip')}</dt>
							<dd data-testid="host-tip" data-tone={frame.tip.tone}>
								{frame.tip.message}
								{#if frame.tip.action}
									<Button size="sm" onclick={() => runTipAction(frame)}
										>{frame.tip.action.label}</Button
									>
								{/if}
							</dd>
						{/if}
						{#if frame.dirty}
							<dt>{$t('surface.embed.harness.hostDirty')}</dt>
							<dd></dd>
						{/if}
						{#if frame.closeRequests > 0}
							<dt>{$t('surface.embed.harness.hostClosed')}</dt>
							<dd data-testid="host-close-requests">{frame.closeRequests}</dd>
						{/if}
						{#if frame.opened.length > 0}
							<dt>{$t('surface.embed.harness.hostOpened')}</dt>
							<dd data-testid="host-opened">{frame.opened.map((item) => item.href).join(', ')}</dd>
						{/if}
					</dl>
					<!-- Same-origin frame: scripts and the origin are kept, top-level navigation is not allowed. -->
					<iframe
						bind:this={frame.element}
						bind:clientWidth={frame.measured}
						class="frame"
						style:width={frame.fixed === null ? '100%' : `${frame.fixed}px`}
						title={frame.fixed === null
							? $t('surface.embed.harness.frameFull')
							: $t('surface.embed.harness.frameWidth', { values: { width: frame.fixed } })}
						sandbox="allow-same-origin allow-scripts allow-popups"
						src={`${recordSurface.href(params)}?frame=${frame.id}`}
						data-testid="harness-frame"
						data-frame-id={frame.id}
					></iframe>
				</section>
			{/each}
		</div>

		<section class="log">
			<h2>{$t('surface.embed.harness.logTitle')}</h2>
			{#if log.length === 0}
				<p>{$t('surface.embed.harness.logEmpty')}</p>
			{:else}
				<ol>
					{#each log as entry, index (index)}
						<li>{entry.frameId} {entry.type}</li>
					{/each}
				</ol>
			{/if}
		</section>
	{/if}
</main>

<style>
	.harness {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		padding: var(--rf-space-8);
		color: var(--rf-text);
	}
	h1 {
		margin: 0;
		font-size: var(--rf-size-title);
		font-weight: var(--rf-weight-semibold);
	}
	h2 {
		margin: 0;
		font-size: var(--rf-size-body);
		font-weight: var(--rf-weight-semibold);
	}
	.intro {
		margin: 0;
		color: var(--rf-secondary);
	}
	.controls,
	.actions,
	.width {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.frames {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-start;
		gap: var(--rf-space-8);
	}
	.host {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		min-width: 0;
	}
	.host:last-child {
		flex: 1 1 100%;
	}
	.chrome {
		display: grid;
		grid-template-columns: max-content minmax(0, 1fr);
		gap: var(--rf-space-2) var(--rf-space-4);
		margin: 0;
		font-size: var(--rf-size-secondary);
	}
	.chrome dt {
		color: var(--rf-secondary);
	}
	.chrome dd {
		margin: 0;
		overflow-wrap: anywhere;
	}
	.frame {
		height: 640px;
		max-width: 100%;
		/* No border: the frame's inner width is exactly the slot width the host announces. */
		border: 0;
		box-shadow: 0 0 0 var(--rf-border-width) var(--rf-line);
		background: var(--rf-bg);
	}
	.log ol {
		margin: 0;
		padding-inline-start: var(--rf-space-8);
		font-size: var(--rf-size-secondary);
	}
</style>
