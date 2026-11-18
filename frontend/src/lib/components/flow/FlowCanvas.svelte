<script lang="ts">
	// Center canvas: mounts `EditorAdapter` (dynamic import, engine chunk only loads here) into a
	// host div, renders the slash menu overlay, and exposes nest/outdent via a per-block hover
	// handle in addition to the Tab/Shift-Tab keyboard shortcut `EditorAdapter` already wires up.
	//
	// This component owns zero content state itself: `FlowEditorAdapter` writes straight into the
	// Loro doc (owned by the route's `FlowObjectRepository` entry), and the DOM the browser shows
	// is ProseMirror's own render of that doc via `LoroSyncPlugin` -- there is no second copy of
	// block text sitting in this component's `$state`.

	import { rememberEditorSelection, recalledEditorSelection } from '#lib/flow/editor-session.js';
	import type { FlowMark } from '#lib/flow/editor-schema.js';
	import { FLOW_COLLECTION_UI_ENABLED } from '#lib/flow/product-mode.js';
	import { onDestroy, onMount, getContext, tick } from 'svelte';
	import { FLOW_REPOSITORY_CONTEXT } from '#lib/flow/context-keys.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import type { PageBodyJobs } from '#lib/flow/body/jobs.js';
	import type { BodyInsertionBookmark } from '#lib/flow/body/assets.js';
	import FlowBodyInsert from './FlowBodyInsert.svelte';
	import { BODY_INSERT_OPTIONS } from '#lib/flow/body/labels.js';
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
	import { ChevronDown, ChevronUp, ListTree, Plus, Search, X } from '@lucide/svelte';
	import { outlineTree, type PageOutlineEntry } from '#lib/flow/editor-page-tools.js';

	interface Props {
		doc: LoroDocType;
		scopeId: string;
		pickerScopeId?: string;
		objectId: string;
		/** `view`/`comment`, or write access revoked: no typing, no slash menu, no block handle. */
		readOnly?: boolean;
		recoveringEdits?: boolean;
		canConvert?: boolean;
		/** Remote peers on this document (ephemeral; rendered as decorations only). */
		presence?: Readable<ReadonlyArray<RemotePresence>>;
		/** Receives the local caret/selection for the session's presence frame. */
		onPresenceCursor?: (cursor: PresenceCursor | null) => void;
		/** Comment on the block under the pointer. Uses that block's existing id. */
		onCommentBlock?: (blockId: string) => void;
		onShareBlock?: (blockId: string) => void;
		onTransferBlock?: (blockId: string, kind: 'move' | 'copy') => void;
		canMoveToPage?: boolean;
		onEnsureStructure?: () => Promise<boolean>;
		lifecycleInfo?: () => {
			refs: number;
			generation: number;
			retired: number;
			paused: boolean;
		} | null;
	}

	let {
		doc,
		scopeId,
		pickerScopeId,
		objectId,
		readOnly = false,
		recoveringEdits = false,
		canConvert = true,
		presence,
		onPresenceCursor,
		onCommentBlock,
		onShareBlock,
		onTransferBlock,
		canMoveToPage = false,
		onEnsureStructure,
		lifecycleInfo
	}: Props = $props();
	let peers = $state<ReadonlyArray<RemotePresence>>([]);

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	let bodyJobs = $state<PageBodyJobs | null>(null);
	let bodyInsert = $state<FlowBodyInsert | undefined>();
	let pageBlocks: FlowPageBlocks | undefined = $state();

	let host: HTMLDivElement | undefined = $state();
	let adapter: FlowEditorAdapter | null = null;
	let slashMenu = $state<SlashMenuState>({ active: false, query: '', coords: null });
	let findOpen = $state(false);
	let findInput: HTMLInputElement | undefined = $state();
	let findButton: HTMLButtonElement | undefined;
	let tools: HTMLDivElement | undefined = $state();
	let outlineButton: HTMLButtonElement | undefined = $state();
	let outlinePanel: HTMLElement | undefined = $state();
	let findFromEditor = false;
	let outlineOpen = $state(false);
	let findQuery = $state('');
	let replacement = $state('');
	let replaceOpen = $state(false);
	let emptyPage = $state(false);
	let addingAtEnd = $state(false);
	let found = $state({ current: 0, total: 0 });
	let headings = $state<PageOutlineEntry[]>([]);
	let pointerDrag = $state<{
		pointerId: number;
		sourceId: string;
		x: number;
		y: number;
		active: boolean;
	} | null>(null);
	let pointerDrop = $state<{
		targetId: string;
		placement: 'before' | 'after' | 'inside';
		copy: boolean;
		top: number;
		left: number;
		width: number;
		height: number;
	} | null>(null);
	let pointerHandle: HTMLButtonElement | null = null;
	let suppressHandleClick = false;
	let dropMessage = $state('');
	let inputError = $state('');
	let plainPasteArmed = $state(false);
	let blockMenu = $state<{ pos: number } | null>(null);
	let addTarget = $state<BodyInsertionBookmark | null>(null);
	let insertQuery = $state('');
	let insertSearch: HTMLInputElement | undefined = $state();
	let insertMenu: HTMLDivElement | undefined = $state();
	let insertTrigger: HTMLButtonElement | undefined = $state();
	let insertAnchor = $state<{ left: number; top: number } | null>(null);
	let bodyReady = $state(false);
	let menuElement: HTMLDivElement | undefined = $state();
	let menuTrigger: HTMLButtonElement | undefined;
	const toolButton =
		'inline-flex disabled:opacity-50 min-h-8 items-center justify-center gap-1.5 rounded-md px-2 text-xs font-medium text-slate-700 hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-blue-600 aria-expanded:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800 dark:aria-expanded:bg-slate-800';
	const toolIcon =
		'grid size-8 shrink-0 place-items-center rounded-md hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-blue-600 disabled:opacity-40 dark:hover:bg-slate-800';
	const toolInput =
		'min-w-0 flex-1 rounded-md border border-slate-300 bg-transparent px-2 py-1.5 text-sm focus-visible:outline-2 focus-visible:outline-blue-600 dark:border-slate-600';
	const toolPanel =
		'absolute right-0 top-full mt-1 max-w-full rounded-lg border border-slate-200 bg-white text-slate-900 shadow-lg dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100';
	const menuClass =
		'fixed z-50 w-48 max-w-[calc(100vw-1rem)] overflow-y-auto overscroll-contain rounded-md border border-slate-200 bg-white p-1 shadow-lg dark:border-slate-700 dark:bg-slate-900';
	const menuItemClass =
		'block w-full rounded px-3 py-1.5 text-left text-sm hover:bg-slate-100 focus:bg-slate-100 dark:hover:bg-slate-800 dark:focus:bg-slate-800';

	$effect(() => {
		if (readOnly) {
			closeMenu();
			closeInsert();
		}
	});

	$effect(() => {
		if (recoveringEdits && addingAtEnd) closeInsert(true);
	});

	function closeMenu(restoreFocus = false): void {
		const trigger = menuTrigger;
		menuTrigger = undefined;
		blockMenu = null;
		if (restoreFocus) trigger?.focus({ preventScroll: true });
	}

	async function openFind(fromEditor = false): Promise<void> {
		findFromEditor = fromEditor;
		closeOutline();
		findOpen = true;
		await tick();
		findInput?.focus({ preventScroll: true });
		if (fromEditor) tools?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
	}

	function closeFind(restoreFocus = true): void {
		if (!findOpen) return;
		findOpen = false;
		findQuery = '';
		replaceOpen = false;
		replacement = '';
		found = adapter?.findText('') ?? { current: 0, total: 0 };
		if (restoreFocus) {
			if (findFromEditor) adapter?.focusEditor();
			else findButton?.focus({ preventScroll: true });
		}
	}

	function findText(query: string, direction?: 1 | -1): void {
		found = adapter?.findText(query, direction) ?? { current: 0, total: 0 };
	}

	function closeOutline(restoreFocus = false): void {
		outlineOpen = false;
		if (restoreFocus) outlineButton?.focus({ preventScroll: true });
	}

	async function toggleOutline(): Promise<void> {
		if (outlineOpen) {
			closeOutline(true);
			return;
		}
		closeFind(false);
		outlineOpen = true;
		await tick();
		outlinePanel?.querySelector('button')?.focus({ preventScroll: true });
	}

	async function jumpToHeading(pos: number): Promise<void> {
		closeOutline();
		await tick();
		adapter?.selectPosition(pos);
	}

	function refreshPageTools(): void {
		headings = outlineTree(adapter?.headings() ?? []);
		if (!headings.length && outlineOpen) {
			closeOutline();
			if (outlinePanel?.contains(document.activeElement))
				findButton?.focus({ preventScroll: true });
		}
		emptyPage = adapter?.isEmptyPage() ?? false;
		if (findOpen) found = adapter?.refreshFind() ?? { current: 0, total: 0 };
	}

	async function placeMenu(trigger: HTMLButtonElement): Promise<void> {
		menuTrigger = trigger;
		await tick();
		const menu = menuElement;
		if (!menu || menuTrigger !== trigger) return;
		const anchor = trigger.getBoundingClientRect();
		const margin = 8;
		const maxHeight = Math.max(0, Math.min(384, window.innerHeight - margin * 2));
		menu.style.maxHeight = `${maxHeight}px`;
		const height = menu.offsetHeight;
		const top =
			anchor.bottom + 4 + height <= window.innerHeight - margin
				? anchor.bottom + 4
				: anchor.top - height - 4;
		menu.style.top = `${Math.max(margin, Math.min(top, window.innerHeight - margin - height))}px`;
		menu.style.left = `${Math.max(margin, Math.min(anchor.left, window.innerWidth - margin - menu.offsetWidth))}px`;
		menu.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus({ preventScroll: true });
	}

	function onMenuKeyDown(event: KeyboardEvent): void {
		if (event.key === 'Escape' || event.key === 'Tab') {
			if (event.key === 'Escape') event.preventDefault();
			closeMenu(true);
			return;
		}
		if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
		const items = Array.from(
			menuElement?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? []
		);
		if (!items.length) return;
		event.preventDefault();
		const current = items.indexOf(document.activeElement as HTMLButtonElement);
		const next =
			event.key === 'Home'
				? 0
				: event.key === 'End'
					? items.length - 1
					: (current + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
		items[next].focus({ preventScroll: true });
		items[next].scrollIntoView({ block: 'nearest', inline: 'nearest' });
	}

	function closeInsert(restoreFocus = false): void {
		const trigger = insertTrigger;
		insertTrigger = undefined;
		addingAtEnd = false;
		addTarget = null;
		insertAnchor = null;
		if (restoreFocus) {
			if (trigger?.isConnected && !trigger.disabled) trigger.focus({ preventScroll: true });
			else adapter?.focusEditor();
		}
	}

	async function openInsert(
		target: BodyInsertionBookmark | null,
		anchor: HTMLElement,
		trigger?: HTMLButtonElement
	): Promise<void> {
		if (readOnly || !target) return;
		if (trigger && trigger === insertTrigger) {
			closeInsert(true);
			return;
		}
		closeMenu();
		closeOutline();
		inputError = '';
		adapter?.dismissSlash();
		slashMenu = { active: false, query: '', coords: null };
		insertTrigger = trigger;
		addingAtEnd = trigger?.dataset.testid === 'flow-add-content';
		const rect = anchor.getBoundingClientRect();
		insertAnchor = { left: rect.left, top: rect.bottom };
		activeOption = 0;
		insertQuery = '';
		addTarget = target;
		await tick();
		if (addTarget) insertSearch?.focus({ preventScroll: true });
	}

	function toggleAddMenu(event: MouseEvent): void {
		if (recoveringEdits) return;
		const trigger = event.currentTarget as HTMLButtonElement;
		void openInsert(adapter?.endBookmark() ?? null, trigger, trigger);
	}

	function currentInsertTarget(): BodyInsertionBookmark | null {
		if (!addingAtEnd) return addTarget;
		return recoveringEdits ? null : (adapter?.endBookmark() ?? null);
	}

	function insertAtHoveredBlock(event: MouseEvent): void {
		const trigger = event.currentTarget as HTMLButtonElement;
		const target =
			hoverBlockId === null
				? (adapter?.bodyBookmark(true) ?? null)
				: { blockId: hoverBlockId, side: event.altKey ? ('before' as const) : ('after' as const) };
		void openInsert(target, trigger, trigger);
	}
	let activeOption = $state(0);
	let selection = $state<ReturnType<FlowEditorAdapter['selectionInfo']>>(null);
	let linkOpen = $state(false);
	let linkHref = $state('');
	let linkError = $state('');
	let hoverBlockId = $state<string | null>(null),
		blockMenuId = $state<string | null>(null);
	let hoverHandle = $state<{ top: number; left: number; pos: number } | null>(null);
	let canvas: HTMLDivElement | undefined = $state();

	const SLASH_OPTIONS: ReadonlyArray<{ type: FlowBlockType; level?: number; labelKey: string }> = [
		{ type: 'paragraph', labelKey: 'flow.canvas.slashMenu.paragraph' },
		{ type: 'heading', level: 1, labelKey: 'flow.canvas.slashMenu.heading1' },
		{ type: 'heading', level: 2, labelKey: 'flow.canvas.slashMenu.heading2' },
		{ type: 'heading', level: 3, labelKey: 'flow.canvas.slashMenu.heading3' },
		{ type: 'bulletItem', labelKey: 'flow.canvas.slashMenu.bullet' },
		{ type: 'codeBlock', labelKey: 'flow.canvas.slashMenu.code' },
		{ type: 'orderedItem', labelKey: 'flow.canvas.slashMenu.ordered' },
		{ type: 'todoItem', labelKey: 'flow.canvas.slashMenu.todo' },
		{ type: 'blockquote', labelKey: 'flow.canvas.slashMenu.quote' },
		{ type: 'toggle', labelKey: 'flow.canvas.slashMenu.toggle' },
		{ type: 'divider', labelKey: 'flow.canvas.slashMenu.divider' }
	];

	const commandQuery = $derived((addTarget ? insertQuery : slashMenu.query).trim().toLowerCase());
	const commandAnchor = $derived(addTarget ? insertAnchor : slashMenu.coords);
	const commandListId = $derived(`flow-block-catalogue-${objectId}`);
	const BODY_OPTIONS = BODY_INSERT_OPTIONS;
	const filteredBody = $derived(
		BODY_OPTIONS.filter((option) =>
			($t(option.labelKey) + ' ' + option.action + ' ' + $t(option.searchAliasesKey))
				.toLowerCase()
				.includes(commandQuery)
		)
	);
	function openBody(
		action: (typeof BODY_OPTIONS)[number]['action'],
		target?: BodyInsertionBookmark
	): void {
		if (readOnly) return;
		const captured = target ?? adapter?.bodyBookmark();
		if (!captured) return;
		bodyInsert?.open(
			action === 'child' ? 'child' : action === 'link' ? 'link' : 'upload',
			captured,
			action === 'attachment' ? 'attachment' : 'image'
		);
	}
	function pickBody(action: (typeof BODY_OPTIONS)[number]['action']): void {
		const target = addTarget ? currentInsertTarget() : adapter?.bodyBookmark();
		if (!target) return;
		if (addTarget) closeInsert(true);
		else adapter?.consumeSlashTrigger();
		slashMenu = { active: false, query: '', coords: null };
		openBody(action, target);
	}

	const filteredOptions = $derived(
		SLASH_OPTIONS.filter((option) =>
			($t(option.labelKey) + ' ' + option.labelKey + ' ' + option.type)
				.toLowerCase()
				.includes(commandQuery)
		)
	);
	// Entries whose result is not a text block: a server-created Collection embed, a Forms
	// Bridge Reference / Embed, and the conversion wizard. Each carries its ownership note.
	const filteredBridgeOptions = $derived(
		SLASH_BRIDGE_OPTIONS.filter(
			(option) =>
				(FLOW_COLLECTION_UI_ENABLED || option.action !== 'collection') &&
				(option.action !== 'convert' || canConvert)
		).filter(
			(option) =>
				commandQuery === '' ||
				(
					$t(option.labelKey) +
					' ' +
					option.labelKey +
					' ' +
					('type' in option ? option.type : option.action)
				)
					.toLowerCase()
					.includes(commandQuery) ||
				option.action.includes(commandQuery)
		)
	);
	const commandCount = $derived(
		filteredOptions.length + filteredBody.length + filteredBridgeOptions.length
	);

	onMount(() => {
		const editorHost = host;
		editorHost?.addEventListener('focusin', showSelectedBlockHandle);
		bodyJobs = repository.bodyJobs(objectId);
		adapter = new FlowEditorAdapter(doc, {
			body: { objectId, jobs: bodyJobs },
			onClipboardError: (reason) => {
				inputError =
					reason === 'cut'
						? $t('flow.canvas.cutUnavailable')
						: $t('flow.canvas.blockPasteUnavailable');
			},
			onBodyAction: (mode, blockId, kind, attrs) => {
				if (!readOnly) bodyInsert?.open(mode, { blockId, side: 'after' }, kind, attrs);
			},
			ensureStructure: () => onEnsureStructure?.() ?? Promise.resolve(false),
			lifecycleInfo: () => lifecycleInfo?.() ?? null,
			onInputError: (message) => (inputError = message),
			onPasteModeChange: (plain) => (plainPasteArmed = plain),
			onSlashMenu: (state) => {
				if (state.query !== slashMenu.query) activeOption = 0;
				slashMenu = state;
			},
			onSelectionChange: () => {
				selection = adapter?.selectionInfo() ?? null;
				if (host?.contains(document.activeElement)) showSelectedBlockHandle();
			},
			onDocumentChange: refreshPageTools,
			onKeyDown: (event) => {
				if (
					(event.metaKey || event.ctrlKey) &&
					event.key === 'k' &&
					selection &&
					!selection.empty
				) {
					linkOpen = true;
					linkHref = selection.linkHref ?? '';
					return true;
				}
				if ((event.metaKey || event.ctrlKey) && event.key === 'f') {
					void openFind(true);
					return true;
				}
				if (!readOnly && event.altKey && event.key === 'Enter' && host) {
					const anchor = adapter?.selectedBlockElement()?.element ?? host;
					void openInsert(adapter?.bodyBookmark(true) ?? null, anchor);
					return true;
				}
				return slashMenu.active && !addTarget && handleCommandKey(event);
			},
			onPresenceCursor: (cursor) => onPresenceCursor?.(cursor)
		});
		adapter.setEditable(!readOnly);
		const mounted = adapter;
		if (host)
			void mounted.mount(host, 'root').then(() => {
				mounted.setEditable(!readOnly);
				mounted.setRemotePresence(peers);
				bodyReady = true;
				refreshPageTools();
				const saved = recalledEditorSelection(doc);
				if (saved) mounted.restoreSelection(saved);
			});
		const unsubscribe = presence?.subscribe((value) => {
			peers = value;
			mounted.setRemotePresence(value);
		});
		return () => {
			editorHost?.removeEventListener('focusin', showSelectedBlockHandle);
			unsubscribe?.();
			const saved = adapter?.getSelection();
			if (saved) rememberEditorSelection(doc, saved);
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
		cancelPointerDrag();
		const saved = adapter?.getSelection();
		if (saved) rememberEditorSelection(doc, saved);
		void adapter?.destroy();
	});

	function handleCommandKey(event: KeyboardEvent): boolean {
		if (event.key === 'Escape') {
			if (addTarget) closeInsert(true);
			else {
				adapter?.dismissSlash();
				slashMenu = { active: false, query: '', coords: null };
			}
			return true;
		}
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			if (commandCount) {
				activeOption =
					(activeOption + (event.key === 'ArrowDown' ? 1 : -1) + commandCount) % commandCount;
				void tick().then(() =>
					document
						.getElementById(`${commandListId}-${activeOption}`)
						?.scrollIntoView({ block: 'nearest' })
				);
			}
			return true;
		}
		if (event.key !== 'Enter' || !commandCount) return false;
		const option = filteredOptions[activeOption];
		if (option) pick(option);
		else if (activeOption < filteredOptions.length + filteredBody.length)
			pickBody(filteredBody[activeOption - filteredOptions.length].action);
		else {
			const bridge =
				filteredBridgeOptions[activeOption - filteredOptions.length - filteredBody.length];
			if (bridge) pickBridge(bridge);
		}
		return true;
	}

	function pick(option: (typeof SLASH_OPTIONS)[number]): void {
		if (readOnly) return;
		if (addTarget) {
			const target = currentInsertTarget();
			closeInsert(!target);
			if (!target) return;
			void adapter?.insertBlockType(target, option.type, option.level).then((success) => {
				if (!success) {
					adapter?.focusEditor();
					inputError = $t('flow.body.error.position');
				}
			});
		} else adapter?.setBlockType(option.type, option.level);
		slashMenu = { active: false, query: '', coords: null };
	}

	function pickBridge(option: SlashBridgeOption): void {
		if (addTarget) closeInsert(true);
		else adapter?.consumeSlashTrigger();
		slashMenu = { active: false, query: '', coords: null };
		pageBlocks?.open(option.action);
	}

	function commentOnHover(): void {
		if (!adapter || hoverHandle === null || !onCommentBlock) return;
		const blockId = hoverBlockId;
		if (blockId) onCommentBlock(blockId);
	}

	function cancelPointerDrag(suppress = true): void {
		const gesture = pointerDrag,
			handle = pointerHandle;
		if (gesture?.active && suppress) suppressHandleClick = true;
		pointerDrag = null;
		pointerDrop = null;
		pointerHandle = null;
		if (gesture && handle?.hasPointerCapture(gesture.pointerId))
			handle.releasePointerCapture(gesture.pointerId);
	}
	function startPointerDrag(event: PointerEvent): void {
		if (event.button !== 0 || !event.isPrimary || readOnly || !adapter || !hoverHandle) return;
		const sourceId = hoverBlockId;
		if (!sourceId || adapter.positionForBlock(sourceId) === null) return;
		pointerHandle = event.currentTarget as HTMLButtonElement;
		pointerDrag = {
			pointerId: event.pointerId,
			sourceId,
			x: event.clientX,
			y: event.clientY,
			active: false
		};
		suppressHandleClick = false;
		pointerHandle.setPointerCapture(event.pointerId);
	}
	function updatePointerDrag(event: PointerEvent): void {
		const gesture = pointerDrag;
		if (!gesture || event.pointerId !== gesture.pointerId || !host || !adapter) return;
		if (readOnly || !(event.buttons & 1)) {
			cancelPointerDrag();
			return;
		}
		if (!gesture.active && Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) < 6)
			return;
		if (!gesture.active) {
			pointerDrag = { ...gesture, active: true };
			closeMenu();
		}
		event.preventDefault();
		updatePointerDrop(event);
	}
	function updatePointerDrop(event: PointerEvent): void {
		if (!host || !pointerDrag) return;
		const copy = event.altKey;
		const element = host.ownerDocument
			.elementFromPoint(event.clientX, event.clientY)
			?.closest<HTMLElement>('[data-block-id]');
		if (
			!element ||
			!host.contains(element) ||
			!element.dataset.blockId ||
			(!copy && element.dataset.blockId === pointerDrag.sourceId)
		) {
			pointerDrop = null;
			return;
		}
		const rect = element.getBoundingClientRect(),
			canvas = host.parentElement!.getBoundingClientRect();
		const placement =
			event.shiftKey && !copy
				? 'inside'
				: event.clientY < rect.top + rect.height / 2
					? 'before'
					: 'after';
		pointerDrop = {
			targetId: element.dataset.blockId,
			placement,
			copy,
			left: rect.left - canvas.left,
			top: (placement === 'after' ? rect.bottom : rect.top) - canvas.top,
			width: rect.width,
			height: placement === 'inside' ? rect.height : 2
		};
	}
	function finishPointerDrag(event: PointerEvent): void {
		const gesture = pointerDrag;
		if (!gesture || event.pointerId !== gesture.pointerId) return;
		if (gesture.active) updatePointerDrop(event);
		const drop = pointerDrop;
		cancelPointerDrag();
		if (!gesture.active || !drop || readOnly || !adapter) return;
		event.preventDefault();
		const editor = adapter;
		const operation =
			drop.copy && drop.placement !== 'inside'
				? editor.copyBlockByIds(gesture.sourceId, drop.targetId, drop.placement)
				: editor.moveBlockByIds(gesture.sourceId, drop.targetId, drop.placement);
		dropMessage = '';
		void operation.then((success) => {
			if (adapter === editor)
				dropMessage = success
					? $t(drop.copy ? 'flow.canvas.blockCopied' : 'flow.canvas.blockMoved')
					: $t(drop.copy ? 'flow.canvas.blockCopyUnavailable' : 'flow.canvas.blockMoveUnavailable');
		});
	}
	$effect(() => {
		if (readOnly && pointerDrag) cancelPointerDrag();
	});

	function showBlockHandle(pos: number, element: HTMLElement | null, fallbackTop: number): void {
		if (!adapter || !canvas || !host) return;
		const canvasRect = canvas.getBoundingClientRect();
		const blockRect = element?.getBoundingClientRect();
		hoverBlockId = element?.dataset.blockId ?? adapter.blockIdAtPos(pos);
		if (!hoverBlockId && !adapter.bodyBookmark(true)) {
			hoverHandle = null;
			return;
		}
		hoverHandle = {
			pos,
			left: Math.max(
				4,
				(blockRect?.left ?? host.getBoundingClientRect().left) - canvasRect.left - 76
			),
			top: Math.max(0, (blockRect?.top ?? fallbackTop) - canvasRect.top)
		};
	}

	function showSelectedBlockHandle(): void {
		if (pointerDrag || blockMenu || addTarget || readOnly) return;
		const block = adapter?.selectedBlockElement();
		if (block) showBlockHandle(block.pos, block.element, block.element.getBoundingClientRect().top);
	}

	function onHostMouseMove(event: MouseEvent): void {
		if (pointerDrag || blockMenu || addTarget) return;
		if (!adapter || !host || (readOnly && !onCommentBlock && !onTransferBlock)) return;
		const target = event.target as HTMLElement;
		if (!host.contains(target)) return;
		const element =
			target.closest<HTMLElement>('[data-block-id]') ??
			Array.from(host.querySelector('.ProseMirror')?.children ?? []).find(
				(block): block is HTMLElement => block instanceof HTMLElement && block.contains(target)
			) ??
			null;
		const pos = element?.dataset.blockId
			? adapter.positionForBlock(element.dataset.blockId)
			: adapter.posAtClientPoint(event.clientX, event.clientY);
		if (pos == null) hoverHandle = null;
		else showBlockHandle(pos, element, event.clientY);
	}

	function onHostMouseLeave(): void {
		if (pointerDrag || blockMenu || addTarget) return;
		if (host?.contains(document.activeElement)) showSelectedBlockHandle();
		else hoverHandle = null;
	}
</script>

<svelte:window
	onkeydown={(event) => {
		if (pointerDrag && (event.key === 'Escape' || event.key === 'Tab')) {
			if (event.key === 'Escape') event.preventDefault();
			cancelPointerDrag();
		} else if (
			event.key === 'Escape' &&
			!event.defaultPrevented &&
			!event.isComposing &&
			(tools?.contains(event.target as Node) || host?.contains(event.target as Node))
		) {
			if (outlineOpen) {
				event.preventDefault();
				closeOutline(true);
			} else if (findOpen) {
				event.preventDefault();
				closeFind();
			}
		}
	}}
	onresize={() => {
		closeMenu();
		if (addTarget) closeInsert(true);
	}}
	onpointerdown={(event) => {
		const target = event.target as Node | null;
		if (target && outlineOpen && !tools?.contains(target)) closeOutline();
		if (target && menuTrigger && !menuTrigger.contains(target) && !menuElement?.contains(target))
			closeMenu();
		if (target && addTarget && !insertTrigger?.contains(target) && !insertMenu?.contains(target))
			closeInsert();
	}}
	onblur={() => {
		cancelPointerDrag();
		closeMenu();
		closeInsert();
	}}
/>

<div
	bind:this={canvas}
	class="relative min-h-[40vh] shrink-0 py-2 pb-16 pr-4 pl-[5rem] sm:pr-8 2xl:pr-16"
	role="presentation"
	onmousemove={onHostMouseMove}
	onmouseleave={onHostMouseLeave}
	data-testid="flow-canvas"
	data-block-dragging={pointerDrag?.active ? 'true' : 'false'}
	data-read-only={readOnly ? 'true' : 'false'}
>
	<span id={`flow-block-drag-help-${objectId}`} class="sr-only"
		>{$t('flow.canvas.blockDragHelp')}</span
	>
	{#if pointerDrag?.active && pointerDrop}<div
			data-testid="flow-block-drop-indicator"
			data-placement={pointerDrop.placement}
			data-operation={pointerDrop.copy ? 'copy' : 'move'}
			data-target-block-id={pointerDrop.targetId}
			class="pointer-events-none absolute z-30 rounded border-2 border-blue-500 bg-blue-100/20"
			style={`top:${pointerDrop.top}px;left:${pointerDrop.left}px;width:${pointerDrop.width}px;height:${pointerDrop.height}px`}
			aria-hidden="true"
		></div>{/if}

	{#snippet outlineItems(items: PageOutlineEntry[])}
		<ol class="space-y-0.5">
			{#each items as item (item.pos)}
				<li>
					<button
						type="button"
						class="w-full rounded px-2 py-1.5 text-start text-sm break-words hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-blue-600 dark:hover:bg-slate-800"
						onclick={() => void jumpToHeading(item.pos)}
					>
						<span class="sr-only"
							>{$t('flow.canvas.outline.headingLevel', { values: { level: item.level } })}
						</span>
						{item.text.trim() || $t('flow.canvas.outline.untitledHeading')}
					</button>
					{#if item.children.length}
						<div class="ms-2 border-s border-slate-200 ps-2 dark:border-slate-700">
							{@render outlineItems(item.children)}
						</div>
					{/if}
				</li>
			{/each}
		</ol>
	{/snippet}
	<div
		bind:this={tools}
		class="sticky top-0 z-40 mx-auto mb-2 max-w-3xl"
		data-testid="flow-page-tools"
	>
		<div class="flex flex-wrap justify-end gap-0.5 bg-white py-0.5 dark:bg-slate-950">
			<button
				type="button"
				class={toolButton}
				bind:this={findButton}
				data-testid="flow-page-find"
				disabled={!bodyReady}
				aria-expanded={findOpen}
				aria-controls={findOpen ? `flow-find-${objectId}` : undefined}
				onclick={() => (findOpen ? closeFind() : void openFind())}
			>
				<Search size={14} aria-hidden="true" />
				{$t('flow.canvas.find.label')}
			</button>
			{#if headings.length}
				<button
					type="button"
					class={toolButton}
					bind:this={outlineButton}
					data-testid="flow-page-outline"
					aria-expanded={outlineOpen}
					aria-controls={outlineOpen ? `flow-outline-${objectId}` : undefined}
					onclick={() => void toggleOutline()}
				>
					<ListTree size={14} aria-hidden="true" />
					{$t('flow.canvas.outline.label')}
				</button>
			{/if}
		</div>
		{#if findOpen}
			<form
				role="search"
				id={`flow-find-${objectId}`}
				aria-label={$t('flow.canvas.find.label')}
				class={`${toolPanel} w-[23rem] space-y-1 px-2 py-1.5`}
				onsubmit={(event) => {
					event.preventDefault();
					findText(findQuery, 1);
				}}
			>
				<div class="flex flex-wrap items-center gap-0.5">
					<input
						type="search"
						class={toolInput}
						aria-label={$t('flow.canvas.find.query')}
						placeholder={$t('flow.canvas.find.query')}
						aria-describedby={`flow-find-scope-${objectId}`}
						bind:this={findInput}
						bind:value={findQuery}
						oninput={(event) => findText(event.currentTarget.value)}
						onkeydown={(event) => {
							if (event.key !== 'Enter' || event.isComposing) return;
							event.preventDefault();
							findText(findQuery, event.shiftKey ? -1 : 1);
						}}
					/>
					<button
						type="button"
						class={toolIcon}
						disabled={!found.total}
						aria-label={$t('flow.canvas.find.previous')}
						title={$t('flow.canvas.find.previous')}
						onmousedown={(event) => event.preventDefault()}
						onclick={() => findText(findQuery, -1)}
						><ChevronUp size={16} aria-hidden="true" /></button
					>
					<button
						type="button"
						class={toolIcon}
						disabled={!found.total}
						aria-label={$t('flow.canvas.find.next')}
						title={$t('flow.canvas.find.next')}
						onmousedown={(event) => event.preventDefault()}
						onclick={() => findText(findQuery, 1)}
						><ChevronDown size={16} aria-hidden="true" /></button
					>
					<button
						type="button"
						class={toolIcon}
						aria-label={$t('flow.canvas.find.close')}
						title={$t('flow.canvas.find.close')}
						onclick={() => closeFind()}><X size={16} aria-hidden="true" /></button
					>
				</div>
				<div class="flex flex-wrap items-center justify-between gap-x-2">
					<span
						id={`flow-find-scope-${objectId}`}
						class="text-xs text-slate-500 dark:text-slate-400">{$t('flow.canvas.find.scope')}</span
					>
					<span role="status" class="text-xs tabular-nums text-slate-600 dark:text-slate-300">
						{#if findQuery.trim()}
							{found.total
								? $t('flow.canvas.find.position', { values: found })
								: $t('flow.canvas.find.empty')}
						{/if}
					</span>
					{#if !readOnly}
						<button
							type="button"
							class={toolButton}
							aria-expanded={replaceOpen}
							aria-controls={replaceOpen ? `flow-replace-${objectId}` : undefined}
							onclick={() => (replaceOpen = !replaceOpen)}>{$t('flow.canvas.find.replace')}</button
						>
					{/if}
				</div>
				{#if replaceOpen && !readOnly}
					<div id={`flow-replace-${objectId}`} class="flex items-center gap-1">
						<input
							class={toolInput}
							aria-label={$t('flow.canvas.find.replacement')}
							placeholder={$t('flow.canvas.find.replacement')}
							bind:value={replacement}
							onkeydown={(event) => {
								if (event.key !== 'Enter' || event.isComposing) return;
								event.preventDefault();
								adapter?.replaceFound(replacement);
							}}
						/>
						<button
							type="button"
							class={toolButton}
							disabled={!found.total}
							onclick={() => adapter?.replaceFound(replacement)}
							>{$t('flow.canvas.find.replace')}</button
						>
					</div>
				{/if}
			</form>
		{:else if outlineOpen}
			<nav
				bind:this={outlinePanel}
				id={`flow-outline-${objectId}`}
				aria-label={$t('flow.canvas.outline.label')}
				class={`${toolPanel} max-h-[min(19rem,45dvh)] w-72 overflow-y-auto overscroll-contain p-1`}
			>
				{@render outlineItems(headings)}
			</nav>
		{/if}
	</div>
	{#if plainPasteArmed}<div
			role="status"
			class="mx-auto mb-2 flex max-w-3xl items-center gap-2 text-xs text-slate-500"
		>
			<span>{$t('flow.canvas.plainPasteHint')}</span><button
				type="button"
				data-flow-plain-paste-control
				onclick={() => adapter?.cancelPlainPaste()}>{$t('flow.recovery.dismiss')}</button
			>
		</div>{/if}

	{#if inputError}<div
			role="alert"
			class="mx-auto mb-3 flex max-w-3xl items-center gap-2 rounded border border-amber-200 bg-amber-50 p-2 text-sm text-amber-900"
		>
			<span class="flex-1">{$t('flow.canvas.editNotApplied')}</span><button
				type="button"
				aria-label={$t('flow.recovery.dismiss')}
				onclick={() => (inputError = '')}>×</button
			>
		</div>{/if}
	{#if dropMessage}<p role="status" class="sr-only">{dropMessage}</p>{/if}
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
	{#if hoverHandle && (!readOnly || onCommentBlock || onTransferBlock)}
		<div
			data-flow-block-controls
			class="absolute z-20 flex items-center gap-0.5 rounded bg-white dark:bg-slate-950"
			style={`top: ${hoverHandle.top}px;left:${hoverHandle.left}px`}
		>
			{#if !readOnly}
				<button
					type="button"
					data-testid="flow-block-insert"
					class="grid size-6 place-items-center rounded text-xl leading-none text-slate-600 hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-300 dark:hover:bg-slate-800"
					aria-label={$t('flow.canvas.slashMenu.insert')}
					aria-keyshortcuts="Alt+Enter"
					aria-haspopup="listbox"
					aria-expanded={!!addTarget && insertTrigger?.dataset.testid === 'flow-block-insert'}
					title={$t('flow.canvas.slashMenu.insertHelp')}
					onclick={insertAtHoveredBlock}><span aria-hidden="true">+</span></button
				>
			{/if}
			{#if onCommentBlock}
				<button
					type="button"
					class="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-700 dark:hover:bg-slate-800 dark:hover:text-slate-200"
					onclick={commentOnHover}
					aria-label={$t('flow.comments.onBlock')}
					title={$t('flow.comments.onBlock')}
				>
					💬
				</button>
			{/if}
			{#if readOnly && onTransferBlock}
				<button
					type="button"
					class="rounded p-1 text-xs underline"
					onclick={() => {
						if (hoverBlockId) onTransferBlock?.(hoverBlockId, 'copy');
					}}>{$t('flow.transfer.copyToPage')}</button
				>
			{/if}
			{#if !readOnly}
				<button
					type="button"
					class="rounded p-1 text-slate-400 hover:bg-slate-100 dark:hover:bg-slate-800"
					draggable="false"
					style="touch-action:none"
					ondragstart={(event) => event.preventDefault()}
					onpointerdown={startPointerDrag}
					onpointermove={updatePointerDrag}
					onpointerup={finishPointerDrag}
					onpointercancel={() => cancelPointerDrag()}
					onlostpointercapture={() => cancelPointerDrag()}
					aria-describedby={`flow-block-drag-help-${objectId}`}
					aria-label={$t('flow.block.menuLabel')}
					title={$t('flow.canvas.blockDragHelp')}
					aria-haspopup="menu"
					aria-expanded={blockMenu !== null}
					onclick={(event) => {
						if (suppressHandleClick) {
							event.preventDefault();
							suppressHandleClick = false;
							return;
						}
						const wasOpen = blockMenu !== null;
						closeMenu();
						if (wasOpen || !hoverHandle) return;
						blockMenu = { pos: hoverHandle.pos };
						blockMenuId = hoverBlockId;
						void placeMenu(event.currentTarget as HTMLButtonElement);
					}}>⋮⋮</button
				>
			{/if}
		</div>
	{/if}

	{#if blockMenu && !readOnly}
		<div
			bind:this={menuElement}
			role="menu"
			aria-label={$t('flow.block.menuLabel')}
			class={menuClass}
			onkeydown={onMenuKeyDown}
			tabindex="-1"
		>
			<p class="px-3 py-2 text-xs text-slate-500">{$t('flow.canvas.cutShareNotice')}</p>
			{#each [...(onShareBlock ? ['share block'] : []), ...(onTransferBlock ? [...(canMoveToPage ? ['move to page'] : []), 'copy to page'] : []), 'select', 'copy', 'cut', 'duplicate', 'move up', 'move down', 'nest', 'outdent', 'paste plain text', 'add image', 'add file', 'new child page', 'link page', 'delete'] as action (action)}
				<button
					type="button"
					role="menuitem"
					tabindex="-1"
					class={`${menuItemClass} capitalize`}
					onclick={() => {
						if (blockMenu && (action === 'move to page' || action === 'copy to page')) {
							const id = blockMenuId;
							if (id && adapter?.positionForBlock(id) != null)
								onTransferBlock?.(id, action === 'move to page' ? 'move' : 'copy');
						} else if (blockMenu && action === 'share block') {
							const id = blockMenuId;
							if (id && adapter?.positionForBlock(id) !== null) onShareBlock?.(id);
						} else if (blockMenu && action === 'move up') adapter?.moveBlockStep(blockMenu.pos, -1);
						else if (blockMenu && action === 'move down') adapter?.moveBlockStep(blockMenu.pos, 1);
						else if (blockMenu && action === 'nest') adapter?.adjustIndentAtPos(blockMenu.pos, 1);
						else if (blockMenu && action === 'outdent')
							adapter?.adjustIndentAtPos(blockMenu.pos, -1);
						else if (action === 'paste plain text') adapter?.togglePlainPaste();
						else if (
							blockMenu &&
							['add image', 'add file', 'new child page', 'link page'].includes(action)
						) {
							const id = adapter?.blockIdAtPos(blockMenu.pos);
							if (id)
								openBody(
									action === 'add image'
										? 'image'
										: action === 'add file'
											? 'attachment'
											: action === 'new child page'
												? 'child'
												: 'link',
									{ blockId: id, side: 'after' }
								);
						} else if (blockMenu)
							adapter?.blockAction(
								blockMenu.pos,
								action as 'select' | 'copy' | 'cut' | 'duplicate' | 'delete'
							);
						closeMenu();
					}}
					>{#if action === 'nest'}{$t('flow.block.nest')}{:else if action === 'outdent'}{$t(
							'flow.block.outdent'
						)}{:else}{action === 'move to page'
							? $t('flow.transfer.moveToPage')
							: action === 'copy to page'
								? $t('flow.transfer.copyToPage')
								: action === 'share block'
									? $t('flow.shared.shareBlock')
									: action === 'cut'
										? $t('flow.canvas.cutBlock')
										: action === 'paste plain text'
											? $t('flow.canvas.plainPasteMode')
											: action === 'add image'
												? $t('flow.body.addImage')
												: action === 'add file'
													? $t('flow.body.addFile')
													: action === 'new child page'
														? $t('flow.body.newChild')
														: action === 'link page'
															? $t('flow.body.linkPage')
															: action}{/if}</button
				>
			{/each}
		</div>
	{/if}

	{#if selection && !selection.empty && !readOnly}
		<div
			role="toolbar"
			aria-label="Text formatting"
			data-testid="flow-format-toolbar"
			class="fixed z-40 flex flex-wrap items-center gap-1 rounded-lg border border-slate-200 bg-white p-1 shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left:${Math.max(8, Math.min(selection.left, typeof window === 'undefined' ? 800 : window.innerWidth - 300))}px;top:${Math.max(8, selection.top - 44)}px`}
		>
			{#each ['bold', 'italic', 'strike', 'code'] as mark (mark)}
				<button
					type="button"
					class="rounded px-2 py-1 text-sm hover:bg-slate-100 dark:hover:bg-slate-800"
					aria-label={mark}
					aria-pressed={selection.marks.includes(mark)}
					onmousedown={(event) => event.preventDefault()}
					onclick={() => adapter?.toggleMark(mark as FlowMark)}
					>{mark === 'bold'
						? 'B'
						: mark === 'italic'
							? 'I'
							: mark === 'strike'
								? 'S̶'
								: '‹›'}</button
				>
			{/each}
			<button
				type="button"
				aria-label="Link"
				class="rounded px-2 py-1 text-sm hover:bg-slate-100"
				onmousedown={(event) => event.preventDefault()}
				onclick={() => {
					linkOpen = !linkOpen;
					linkHref = selection?.linkHref ?? '';
					linkError = '';
				}}>Link</button
			>
			{#if linkOpen}
				<form
					class="flex items-center gap-1"
					onsubmit={(event) => {
						event.preventDefault();
						if (adapter?.setLink(linkHref)) {
							linkOpen = false;
						} else linkError = 'Enter a safe web, email or page address';
					}}
				>
					<input
						aria-label="Link address"
						bind:value={linkHref}
						placeholder="https://"
						class="w-44 rounded border px-2 py-1 text-sm"
					/>
					<button type="submit" class="px-2 text-sm">Apply</button>
					<button
						type="button"
						aria-label="Remove link"
						onclick={() => {
							adapter?.setLink('');
							linkOpen = false;
						}}>×</button
					>
					{#if linkError}<span role="alert" class="text-xs text-red-600">{linkError}</span>{/if}
				</form>
			{/if}
		</div>
	{/if}
	<div
		bind:this={host}
		class="flow-canvas prose prose-slate mx-auto max-w-3xl dark:prose-invert focus:outline-none"
		data-placeholder={$t('flow.canvas.placeholder')}
	></div>

	{#if !readOnly && bodyReady}
		<div class="mx-auto max-w-3xl" class:mt-1={emptyPage} class:mt-3={!emptyPage}>
			<button
				type="button"
				class={toolButton}
				aria-haspopup="listbox"
				aria-expanded={!!addTarget && insertTrigger?.dataset.testid === 'flow-add-content'}
				data-testid="flow-add-content"
				disabled={recoveringEdits}
				onclick={toggleAddMenu}
			>
				<Plus size={16} aria-hidden="true" />
				{$t(recoveringEdits ? 'flow.canvas.restoringEdits' : 'flow.body.addContent')}
			</button>
		</div>
	{/if}
	{#if bodyJobs}<FlowBodyInsert
			bind:this={bodyInsert}
			jobs={bodyJobs}
			scopeId={pickerScopeId ?? scopeId}
			{readOnly}
			bookmark={() => adapter?.bodyBookmark(true) ?? null}
		/>{/if}
	<FlowPageBlocks bind:this={pageBlocks} {doc} {scopeId} {objectId} {readOnly} />

	{#if commandAnchor && (addTarget || slashMenu.active) && !readOnly}
		<div
			bind:this={insertMenu}
			data-flow-command-popup
			class="fixed z-50 w-72 max-w-[calc(100vw-1rem)] rounded-md border border-slate-200 bg-white shadow-lg dark:border-slate-700 dark:bg-slate-900"
			style={`left: ${Math.max(8, Math.min(commandAnchor.left, window.innerWidth - 296))}px; top: ${Math.max(8, Math.min(commandAnchor.top + 4, window.innerHeight - 330))}px`}
		>
			{#if addTarget}
				<div class="px-3 pt-2 text-xs font-medium text-slate-600 dark:text-slate-300">
					{$t(
						addTarget.blockId == null
							? 'flow.body.addContent'
							: insertTrigger?.dataset.testid === 'flow-add-content'
								? 'flow.canvas.slashMenu.insertAtEnd'
								: addTarget.side === 'before'
									? 'flow.canvas.slashMenu.insertAbove'
									: 'flow.canvas.slashMenu.insertBelow'
					)}
				</div>
				<input
					bind:this={insertSearch}
					bind:value={insertQuery}
					oninput={() => (activeOption = 0)}
					onkeydown={(event) => {
						if (event.isComposing) return;
						if (event.key === 'Tab') closeInsert(true);
						else if (handleCommandKey(event)) event.preventDefault();
					}}
					role="combobox"
					aria-autocomplete="list"
					aria-expanded="true"
					aria-controls={commandListId}
					aria-activedescendant={commandCount ? `${commandListId}-${activeOption}` : undefined}
					aria-label={$t('flow.canvas.slashMenu.search')}
					placeholder={$t('flow.canvas.slashMenu.search')}
					class="w-full rounded-t-md border-b border-slate-200 bg-transparent px-3 py-2 text-sm text-slate-900 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-500 dark:border-slate-700 dark:text-white"
				/>
			{/if}
			<div
				id={commandListId}
				class="max-h-[min(15rem,calc(100dvh-7rem))] overflow-y-auto overscroll-contain py-1"
				role="listbox"
				aria-label={$t('flow.canvas.slashMenu.insert')}
			>
				{#if filteredOptions.length === 0 && filteredBody.length === 0 && filteredBridgeOptions.length === 0}
					<p class="px-3 py-2 text-sm text-slate-400">{$t('flow.canvas.slashMenu.empty')}</p>
				{:else}
					{#each filteredOptions as option, index (option.labelKey + String(option.level ?? ''))}
						<button
							type="button"
							class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
							onclick={() => pick(option)}
							id={`${commandListId}-${index}`}
							tabindex="-1"
							onmousedown={(event) => event.preventDefault()}
							role="option"
							class:bg-slate-100={activeOption === index}
							class:dark:bg-slate-800={activeOption === index}
							aria-selected={activeOption === index}
						>
							{$t(option.labelKey)}
						</button>
					{/each}
					{#each filteredBody as option, index (option.action)}<button
							type="button"
							role="option"
							id={`${commandListId}-${filteredOptions.length + index}`}
							tabindex="-1"
							onmousedown={(event) => event.preventDefault()}
							aria-selected={activeOption === filteredOptions.length + index}
							class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
							class:bg-slate-100={activeOption === filteredOptions.length + index}
							class:dark:bg-slate-800={activeOption === filteredOptions.length + index}
							onclick={() => pickBody(option.action)}
							data-testid={`flow-slash-${option.action}`}>{$t(option.labelKey)}</button
						>{/each}

					{#if filteredBridgeOptions.length > 0}
						<div
							class="my-1 border-t border-slate-200 dark:border-slate-700"
							role="presentation"
						></div>
						{#each filteredBridgeOptions as option, index (option.action)}
							<button
								type="button"
								class="block w-full px-3 py-1.5 text-left text-sm text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-800"
								onclick={() => pickBridge(option)}
								id={`${commandListId}-${filteredOptions.length + filteredBody.length + index}`}
								tabindex="-1"
								onmousedown={(event) => event.preventDefault()}
								role="option"
								aria-selected={activeOption ===
									filteredOptions.length + filteredBody.length + index}
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
		</div>
	{/if}
</div>

<style>
	.flow-canvas :global(.flow-find-match) {
		background-color: #fef08a;
		color: #422006;
		border-radius: 2px;
	}
	.flow-canvas :global(.flow-find-current) {
		background-color: #fdba74;
		outline: 1px solid #9a3412;
	}

	:global(.flow-canvas .ProseMirror) {
		min-height: 60vh;
		outline: none;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
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
	:global(.flow-canvas .flow-ordered-item::before) {
		content: attr(data-order) '. ';
	}
	:global(.flow-canvas .flow-todo-item::before) {
		content: '☐ ';
		cursor: pointer;
	}
	:global(.flow-canvas .flow-todo-item[data-checked='true']::before) {
		content: '☑ ';
	}
	:global(.flow-canvas .flow-todo-item[data-checked='true']) {
		text-decoration: line-through;
		color: var(--color-slate-500);
	}
	:global(.flow-canvas blockquote) {
		border-left: 3px solid var(--color-slate-300);
		padding-left: 1rem;
	}
	:global(.flow-canvas .flow-toggle::before) {
		content: '▾ ';
		cursor: pointer;
	}
	:global(.flow-canvas .flow-toggle[data-collapsed='true']::before) {
		content: '▸ ';
	}
	:global(.flow-canvas a) {
		color: var(--color-blue-600);
		text-decoration: underline;
	}
</style>
