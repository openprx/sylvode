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
		type MarkFlags,
		type SelectionToolbarState,
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

	let canvasEl: HTMLDivElement | undefined = $state();
	let host: HTMLDivElement | undefined = $state();
	let adapter: FlowEditorAdapter | null = null;
	let slashMenu = $state<SlashMenuState>({ active: false, query: '', coords: null });
	let hoverHandle = $state<{ top: number; index: number } | null>(null);
	let blockMenuOpen = $state(false);
	let dropIndex = $state<number | null>(null);
	let linkOpen = $state(false);
	let linkDraft = $state('');
	const emptyMarks: MarkFlags = {
		bold: false,
		italic: false,
		code: false,
		strike: false,
		link: false
	};
	let toolbar = $state<SelectionToolbarState>({
		active: false,
		coords: null,
		marks: emptyMarks
	});

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
			onPresenceCursor: (cursor) => onPresenceCursor?.(cursor),
			onSelectionToolbar: (state) => {
				toolbar = state;
				if (!state.active) linkOpen = false;
			},
			onLinkRequest: () => {
				linkOpen = true;
			}
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
			blockMenuOpen = false;
			linkOpen = false;
			toolbar = { active: false, coords: null, marks: emptyMarks };
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
		if (!adapter || !canvasEl || readOnly || blockMenuOpen) return;
		const pos = adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos === null) {
			hoverHandle = null;
			return;
		}
		const index = adapter.blockIndexAt(pos);
		if (index === null) {
			hoverHandle = null;
			return;
		}
		const block = (event.target as HTMLElement | null)?.closest?.('.ProseMirror > *');
		const canvasRect = canvasEl.getBoundingClientRect();
		const blockRect = block?.getBoundingClientRect();
		const top = blockRect
			? blockRect.top - canvasRect.top + canvasEl.scrollTop
			: event.clientY - canvasRect.top + canvasEl.scrollTop;
		hoverHandle = { index, top };
	}

	function onHostMouseLeave(): void {
		if (!blockMenuOpen) hoverHandle = null;
	}

	function actedRange(): { start: number; end: number } | null {
		return adapter?.selectedRange() ?? (hoverHandle ? { start: hoverHandle.index, end: hoverHandle.index + 1 } : null);
	}

	function onHandleClick(event: MouseEvent): void {
		if (!adapter || !hoverHandle) return;
		event.preventDefault();
		adapter.selectBlock(hoverHandle.index, event.shiftKey);
	}

	function onHandleDragStart(event: DragEvent): void {
		if (!adapter || !hoverHandle) return;
		const selected = adapter.selectedRange();
		const inside =
			selected !== null && hoverHandle.index >= selected.start && hoverHandle.index < selected.end;
		const start = inside && selected ? selected.start : hoverHandle.index;
		const end = inside && selected ? selected.end : hoverHandle.index + 1;
		event.dataTransfer?.setData('application/x-sylvode-flow-move', JSON.stringify({ start, end }));
		if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
	}

	function draggingBlocks(data: DataTransfer | null): boolean {
		return !!data && Array.from(data.types).includes('application/x-sylvode-flow-move');
	}

	function onCanvasDragOver(event: DragEvent): void {
		if (!draggingBlocks(event.dataTransfer) || !adapter) return;
		event.preventDefault();
		const pos = adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos === null) return;
		const index = adapter.blockIndexAt(pos);
		if (index === null) return;
		const block = (event.target as HTMLElement | null)?.closest?.('.ProseMirror > *');
		const rect = block?.getBoundingClientRect();
		dropIndex = rect && event.clientY > rect.top + rect.height / 2 ? index + 1 : index;
	}

	function onCanvasDrop(event: DragEvent): void {
		event.preventDefault();
		const raw = event.dataTransfer?.getData('application/x-sylvode-flow-move');
		dropIndex = null;
		if (!raw || !adapter) return;
		let parsed: { start?: unknown; end?: unknown };
		try {
			parsed = JSON.parse(raw) as { start?: unknown; end?: unknown };
		} catch {
			return;
		}
		if (typeof parsed.start !== 'number' || typeof parsed.end !== 'number') return;
		const pos = adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos === null) return;
		const index = adapter.blockIndexAt(pos);
		if (index === null) return;
		const block = (event.target as HTMLElement | null)?.closest?.('.ProseMirror > *');
		const rect = block?.getBoundingClientRect();
		const dest = rect && event.clientY > rect.top + rect.height / 2 ? index + 1 : index;
		adapter.moveBlockRange(parsed.start, parsed.end, dest);
	}

	function turnHovered(type: FlowBlockType, level?: number): void {
		const range = actedRange();
		if (!range) return;
		adapter?.setBlockTypeRange(range.start, range.end, type, level);
		blockMenuOpen = false;
	}

	function indentHovered(delta: number): void {
		const range = actedRange();
		if (!range) return;
		adapter?.adjustIndentRange(range.start, range.end, delta);
	}

	function duplicateHovered(): void {
		const range = actedRange();
		if (!range) return;
		adapter?.duplicateBlockRange(range.start, range.end);
		blockMenuOpen = false;
	}

	function deleteHovered(): void {
		const range = actedRange();
		if (!range) return;
		adapter?.deleteBlockRange(range.start, range.end);
		blockMenuOpen = false;
		hoverHandle = null;
	}

	function applyLink(event: SubmitEvent): void {
		event.preventDefault();
		adapter?.setLink(linkDraft.trim());
		linkOpen = false;
		linkDraft = '';
	}

	function removeLink(): void {
		adapter?.setLink(null);
		linkOpen = false;
		linkDraft = '';
	}
</script>

<div
	bind:this={canvasEl}
	class="relative min-h-[50vh] flex-1 overflow-y-auto bg-white px-16 py-10 lg:min-h-0 dark:bg-slate-950"
	role="presentation"
	onmousemove={onHostMouseMove}
	onmouseleave={onHostMouseLeave}
	ondragover={onCanvasDragOver}
	ondrop={onCanvasDrop}
	data-testid="flow-canvas"
	data-read-only={readOnly ? 'true' : 'false'}
	data-drop-index={dropIndex === null ? '' : String(dropIndex)}
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
			class="absolute left-2 z-20 flex items-center gap-0.5"
			style={`top: ${hoverHandle.top}px`}
			data-testid="flow-block-handle"
		>
			<button
				type="button"
				draggable="true"
				class="cursor-grab rounded px-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 active:cursor-grabbing dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={onHandleClick}
				ondragstart={onHandleDragStart}
				aria-label={$t('flow.block.dragLabel')}
				title={$t('flow.block.dragLabel')}
			>
				⋮⋮
			</button>
			<button
				type="button"
				class="rounded px-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
				onclick={() => (blockMenuOpen = !blockMenuOpen)}
				aria-haspopup="menu"
				aria-expanded={blockMenuOpen}
				aria-label={$t('flow.block.menuButton')}
				data-testid="flow-block-menu-button"
			>
				+
			</button>
		</div>
	{/if}
	{#if blockMenuOpen && hoverHandle && !readOnly}
		<div
			class="absolute left-14 z-30 w-48 rounded-md border border-slate-200 bg-white py-1 text-sm shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`top: ${hoverHandle.top}px`}
			role="menu"
			aria-label={$t('flow.block.menuLabel')}
			data-testid="flow-block-menu"
		>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('paragraph')}>{$t('flow.block.paragraph')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('heading', 1)}>{$t('flow.block.heading1')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('heading', 2)}>{$t('flow.block.heading2')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('heading', 3)}>{$t('flow.block.heading3')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('bulletItem')}>{$t('flow.block.bullet')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => turnHovered('codeBlock')}>{$t('flow.block.codeBlock')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => indentHovered(-1)}>{$t('flow.block.outdent')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" onclick={() => indentHovered(1)}>{$t('flow.block.nest')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" data-testid="flow-block-duplicate" onclick={duplicateHovered}>{$t('flow.block.duplicate')}</button>
			<button type="button" class="block w-full px-3 py-1.5 text-left text-red-600 hover:bg-slate-100 dark:hover:bg-slate-800" role="menuitem" data-testid="flow-block-delete" onclick={deleteHovered}>{$t('flow.block.delete')}</button>
		</div>
	{/if}
	{#if toolbar.active && toolbar.coords && !readOnly && !slashMenu.active}
		<div
			class="fixed z-50 flex items-center gap-0.5 rounded-md border border-slate-200 bg-white p-1 shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left: ${toolbar.coords.left}px; top: ${toolbar.coords.top - 40}px`}
			role="toolbar"
			aria-label={$t('flow.block.toolbar')}
			data-testid="flow-selection-toolbar"
		>
			<button type="button" class={`rounded px-2 py-1 text-sm font-semibold ${toolbar.marks.bold ? 'bg-slate-200 dark:bg-slate-700' : ''}`} aria-pressed={toolbar.marks.bold} aria-label={$t('flow.block.bold')} data-testid="flow-mark-bold" onclick={() => adapter?.toggleMark('bold')}>B</button>
			<button type="button" class={`rounded px-2 py-1 text-sm italic ${toolbar.marks.italic ? 'bg-slate-200 dark:bg-slate-700' : ''}`} aria-pressed={toolbar.marks.italic} aria-label={$t('flow.block.italic')} data-testid="flow-mark-italic" onclick={() => adapter?.toggleMark('italic')}>I</button>
			<button type="button" class={`rounded px-2 py-1 text-sm line-through ${toolbar.marks.strike ? 'bg-slate-200 dark:bg-slate-700' : ''}`} aria-pressed={toolbar.marks.strike} aria-label={$t('flow.block.strike')} data-testid="flow-mark-strike" onclick={() => adapter?.toggleMark('strike')}>S</button>
			<button type="button" class={`rounded px-2 py-1 font-mono text-sm ${toolbar.marks.code ? 'bg-slate-200 dark:bg-slate-700' : ''}`} aria-pressed={toolbar.marks.code} aria-label={$t('flow.block.code')} data-testid="flow-mark-code" onclick={() => adapter?.toggleMark('code')}>&lt;/&gt;</button>
			<button type="button" class={`rounded px-2 py-1 text-sm ${toolbar.marks.link ? 'bg-slate-200 dark:bg-slate-700' : ''}`} aria-pressed={toolbar.marks.link} aria-label={$t('flow.block.link')} data-testid="flow-mark-link" onclick={() => (linkOpen = !linkOpen)}>↗</button>
		</div>
	{/if}
	{#if linkOpen && toolbar.coords && !readOnly}
		<form
			class="fixed z-50 flex items-center gap-1 rounded-md border border-slate-200 bg-white p-2 shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left: ${toolbar.coords.left}px; top: ${toolbar.coords.top - 84}px`}
			data-testid="flow-link-form"
			onsubmit={applyLink}
		>
			<input
				bind:value={linkDraft}
				type="url"
				class="w-56 rounded border border-slate-300 px-2 py-1 text-sm dark:border-slate-600 dark:bg-slate-950"
				placeholder="https://"
				aria-label={$t('flow.block.linkUrl')}
				data-testid="flow-link-input"
			/>
			<button type="submit" class="rounded bg-slate-900 px-2 py-1 text-sm text-white dark:bg-slate-100 dark:text-slate-900" data-testid="flow-link-apply">{$t('flow.block.linkApply')}</button>
			<button type="button" class="rounded px-2 py-1 text-sm" data-testid="flow-link-remove" onclick={removeLink}>{$t('flow.block.linkRemove')}</button>
		</form>
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
	:global(.flow-canvas .ProseMirror strong) {
		font-weight: 650;
	}
	:global(.flow-canvas .ProseMirror s) {
		text-decoration: line-through;
	}
	:global(.flow-canvas .ProseMirror a) {
		color: var(--color-blue-600);
		text-decoration: underline;
	}
	:global(.flow-canvas .ProseMirror :not(pre) > code) {
		border-radius: 0.25rem;
		background: var(--color-slate-100);
		padding: 0.05rem 0.25rem;
		font-family: ui-monospace, monospace;
		font-size: 0.9em;
	}
	:global(.dark .flow-canvas .ProseMirror :not(pre) > code) {
		background: var(--color-slate-800);
	}
	:global(.flow-canvas .flow-block-selected) {
		border-radius: 0.25rem;
		background: color-mix(in srgb, var(--color-blue-500) 12%, transparent);
	}
</style>
