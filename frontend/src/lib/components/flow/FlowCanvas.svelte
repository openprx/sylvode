<script lang="ts">
	// Center canvas: mounts `EditorAdapter` (dynamic import, engine chunk only loads here) into a
	// host div, renders the slash menu overlay, and exposes nest/outdent via a per-block hover
	// handle in addition to the Tab/Shift-Tab keyboard shortcut `EditorAdapter` already wires up.
	//
	// This component owns zero content state itself: `FlowEditorAdapter` writes straight into the
	// Loro doc (owned by the route's `FlowObjectRepository` entry), and the DOM the browser shows
	// is ProseMirror's own render of that doc via `LoroSyncPlugin` -- there is no second copy of
	// block text sitting in this component's `$state`.

	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { Readable } from 'svelte/store';
	import {
		FlowEditorAdapter,
		type FlowBlockType,
		type SlashMenuState
	} from '#lib/flow/editor-adapter.js';
	import type { LoroDocType } from 'loro-prosemirror';
	import { SLASH_BRIDGE_OPTIONS, type SlashBridgeOption } from '#lib/flow/slash-bridge.js';
	import {
		safePresenceColor,
		type PresenceCursor,
		type RemotePresence
	} from '#lib/flow/presence.js';
	import FlowPageBlocks from './FlowPageBlocks.svelte';

	interface Props {
		doc: LoroDocType;
		workspaceId: string;
		objectId: string;
		/** `view`/`comment`, or write access revoked: no typing, no slash menu, no block handle. */
		readOnly?: boolean;
		/** Remote peers on this document (ephemeral; rendered as decorations only). */
		presence?: Readable<ReadonlyArray<RemotePresence>>;
		/** Receives the local caret/selection for the session's presence frame. */
		onPresenceCursor?: (cursor: PresenceCursor | null) => void;
	}

	let {
		doc,
		workspaceId,
		objectId,
		readOnly = false,
		presence,
		onPresenceCursor
	}: Props = $props();
	let peers = $state<ReadonlyArray<RemotePresence>>([]);

	let pageBlocks: FlowPageBlocks | undefined = $state();

	let host: HTMLDivElement | undefined = $state();
	let adapter: FlowEditorAdapter | null = null;
	let slashMenu = $state<SlashMenuState>({ active: false, query: '', coords: null });
	let hoverHandle = $state<{ top: number; pos: number } | null>(null);

	const SLASH_OPTIONS: ReadonlyArray<{ type: FlowBlockType; level?: number; labelKey: string }> = [
		{ type: 'paragraph', labelKey: 'flow.canvas.slashMenu.paragraph' },
		{ type: 'heading', level: 1, labelKey: 'flow.canvas.slashMenu.heading1' },
		{ type: 'heading', level: 2, labelKey: 'flow.canvas.slashMenu.heading2' },
		{ type: 'heading', level: 3, labelKey: 'flow.canvas.slashMenu.heading3' },
		{ type: 'bulletItem', labelKey: 'flow.canvas.slashMenu.bullet' },
		{ type: 'codeBlock', labelKey: 'flow.canvas.slashMenu.code' }
	];

	const filteredOptions = $derived(
		SLASH_OPTIONS.filter(
			(option) =>
				option.labelKey.toLowerCase().includes(slashMenu.query.toLowerCase()) ||
				slashMenu.query === ''
		)
	);
	// Entries whose result is not a text block: a server-created Collection embed, a Forms
	// Bridge Reference / Embed, and the conversion wizard. Each carries its ownership note.
	const filteredBridgeOptions = $derived(
		SLASH_BRIDGE_OPTIONS.filter(
			(option) =>
				slashMenu.query === '' ||
				option.labelKey.toLowerCase().includes(slashMenu.query.toLowerCase()) ||
				option.action.includes(slashMenu.query.toLowerCase())
		)
	);

	onMount(() => {
		adapter = new FlowEditorAdapter(doc, {
			onSlashMenu: (state) => {
				slashMenu = state;
			},
			onPresenceCursor: (cursor) => onPresenceCursor?.(cursor)
		});
		adapter.setEditable(!readOnly);
		const mounted = adapter;
		if (host)
			void mounted.mount(host, 'root').then(() => {
				mounted.setEditable(!readOnly);
				mounted.setRemotePresence(peers);
			});
		const unsubscribe = presence?.subscribe((value) => {
			peers = value;
			mounted.setRemotePresence(value);
		});
		return () => {
			unsubscribe?.();
			void adapter?.destroy();
		};
	});

	$effect(() => {
		adapter?.setEditable(!readOnly);
		if (readOnly) {
			slashMenu = { active: false, query: '', coords: null };
			hoverHandle = null;
		}
	});

	onDestroy(() => {
		void adapter?.destroy();
	});

	function pick(option: (typeof SLASH_OPTIONS)[number]): void {
		adapter?.setBlockType(option.type, option.level);
		slashMenu = { active: false, query: '', coords: null };
	}

	function pickBridge(option: SlashBridgeOption): void {
		adapter?.consumeSlashTrigger();
		slashMenu = { active: false, query: '', coords: null };
		pageBlocks?.open(option.action);
	}

	function onHostMouseMove(event: MouseEvent): void {
		if (!adapter || !host || readOnly) return;
		const pos = adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos === null) {
			hoverHandle = null;
			return;
		}
		const hostRect = host.getBoundingClientRect();
		hoverHandle = { pos, top: event.clientY - hostRect.top };
	}

	function onHostMouseLeave(): void {
		hoverHandle = null;
	}

	function adjustHoveredIndent(delta: number): void {
		if (!adapter || hoverHandle === null) return;
		adapter.adjustIndentAtPos(hoverHandle.pos, delta);
	}
</script>

<div
	class="relative min-h-[50vh] flex-1 overflow-y-auto bg-white px-16 py-10 lg:min-h-0 dark:bg-slate-950"
	role="presentation"
	onmousemove={onHostMouseMove}
	onmouseleave={onHostMouseLeave}
	data-testid="flow-canvas"
	data-read-only={readOnly ? 'true' : 'false'}
>
	{#if peers.length > 0}
		<ul
			class="mx-auto mb-3 flex max-w-3xl flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-slate-400"
			aria-label={$t('flow.presence.listLabel')}
			data-testid="flow-presence-list"
		>
			{#each peers as peer (peer.sessionId)}
				<li
					class="flex items-center gap-1"
					data-presence-user={peer.attested ? peer.user.userId : undefined}
				>
					<span
						class="inline-block h-2.5 w-2.5 rounded-full"
						style:background-color={safePresenceColor(peer.user.color, peer.user.userId)}
						aria-hidden="true"
					></span>
					<span>{$t('flow.presence.editing', { values: { name: peer.user.name } })}</span>
				</li>
			{/each}
		</ul>
	{/if}
	{#if readOnly}
		<p
			class="mx-auto mb-3 max-w-3xl text-xs text-slate-500 dark:text-slate-400"
			data-testid="flow-canvas-read-only"
		>
			{$t('flow.canvas.readOnlyNotice')}
		</p>
	{/if}
	{#if hoverHandle && !readOnly}
		<div
			class="absolute -left-1 flex -translate-x-full items-center gap-0.5 opacity-70 hover:opacity-100"
			style={`top: ${hoverHandle.top}px`}
		>
			<button
				type="button"
				class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={() => adjustHoveredIndent(-1)}
				aria-label={$t('flow.block.outdent')}
				title={$t('flow.block.outdent')}
			>
				⇤
			</button>
			<button
				type="button"
				class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={() => adjustHoveredIndent(1)}
				aria-label={$t('flow.block.nest')}
				title={$t('flow.block.nest')}
			>
				⇥
			</button>
			<span
				class="cursor-grab select-none px-1 text-slate-300 dark:text-slate-600"
				aria-label={$t('flow.block.handleLabel')}
			>
				⋮⋮
			</span>
		</div>
	{/if}

	<div
		bind:this={host}
		class="flow-canvas prose prose-slate mx-auto max-w-3xl dark:prose-invert focus:outline-none"
		data-placeholder={$t('flow.canvas.placeholder')}
	></div>

	<FlowPageBlocks bind:this={pageBlocks} {doc} {workspaceId} {objectId} {readOnly} />

	{#if slashMenu.active && slashMenu.coords && !readOnly}
		<div
			class="fixed z-50 w-72 rounded-md border border-slate-200 bg-white py-1 shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left: ${slashMenu.coords.left}px; top: ${slashMenu.coords.top + 4}px`}
			role="listbox"
			aria-label={$t('flow.canvas.slashMenu.paragraph')}
		>
			{#if filteredOptions.length === 0 && filteredBridgeOptions.length === 0}
				<p class="px-3 py-2 text-sm text-slate-400">{$t('flow.canvas.slashMenu.empty')}</p>
			{:else}
				{#each filteredOptions as option (option.labelKey + String(option.level ?? ''))}
					<button
						type="button"
						class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
						onclick={() => pick(option)}
						role="option"
						aria-selected="false"
					>
						{$t(option.labelKey)}
					</button>
				{/each}
				{#if filteredBridgeOptions.length > 0}
					<div
						class="my-1 border-t border-slate-200 dark:border-slate-700"
						role="presentation"
					></div>
					{#each filteredBridgeOptions as option (option.action)}
						<button
							type="button"
							class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
							onclick={() => pickBridge(option)}
							role="option"
							aria-selected="false"
							data-testid={`flow-slash-${option.action}`}
						>
							<span class="block">{$t(option.labelKey)}</span>
							<span class="block text-xs text-slate-500 dark:text-slate-400"
								>{$t(option.noteKey)}</span
							>
						</button>
					{/each}
				{/if}
			{/if}
		</div>
	{/if}
</div>

<style>
	:global(.flow-canvas .ProseMirror) {
		min-height: 60vh;
		outline: none;
	}
	:global(.flow-canvas .ProseMirror p:first-child:last-child:empty::before) {
		content: attr(data-placeholder);
		color: var(--color-slate-400);
		pointer-events: none;
	}
	:global(.flow-canvas [data-indent]) {
		margin-left: calc(var(--flow-indent, 0) * 1.5rem);
	}
	:global(.flow-canvas .flow-bullet-item) {
		position: relative;
		padding-left: 1.25rem;
	}
	:global(.flow-canvas .flow-bullet-item::before) {
		content: '•';
		position: absolute;
		left: 0.25rem;
	}
	:global(.flow-canvas pre) {
		background: var(--color-slate-100);
		border-radius: 0.375rem;
		padding: 0.75rem;
		overflow-x: auto;
	}
	:global(.dark .flow-canvas pre) {
		background: var(--color-slate-900);
	}
	:global(.flow-canvas .flow-presence-caret) {
		position: relative;
		margin-left: -1px;
		margin-right: -1px;
		border-left: 2px solid;
		pointer-events: none;
	}
	:global(.flow-canvas .flow-presence-caret::after) {
		content: attr(data-presence-name);
		background-color: var(--flow-presence-color);
		position: absolute;
		top: -1.35em;
		left: -2px;
		white-space: nowrap;
		border-radius: 0.25rem;
		padding: 0 0.25rem;
		font-size: 0.7rem;
		line-height: 1.2rem;
		color: white;
		user-select: none;
	}
	:global(.flow-canvas .flow-presence-avatar::after) {
		content: attr(data-initials);
	}
	:global(.flow-canvas .flow-presence-avatar) {
		position: absolute;
		left: -2.25rem;
		display: inline-flex;
		height: 1.25rem;
		width: 1.25rem;
		align-items: center;
		justify-content: center;
		border-radius: 9999px;
		font-size: 0.6rem;
		color: white;
		user-select: none;
		pointer-events: none;
	}
	:global(.flow-canvas .ProseMirror > *) {
		position: relative;
	}
</style>
