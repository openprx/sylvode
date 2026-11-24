<script lang="ts">
	import { FLOW_TASK_SESSIONS, type FlowTaskSessions } from '#lib/workbench/flow-sessions.js';
	import { TaskWorkbench, WORKBENCH_CONTEXT } from '#lib/workbench/tabs.js';
	import { observeNavigatorDocument } from '#lib/flow/navigator-doc-observer.js';
	import { navigatorPlacements } from '#lib/flow/navigator-placements.js';
	import { resolve } from '$app/paths';
	// Left navigator: create/select/expand/collapse Page objects, same-parent-only pointer AND
	// keyboard reorder (`ADR-0012` §4: "v0.4 的拖拽只做同一父级内重排"). Reorder is a real CRDT
	// content change on the single per-workspace Navigator object's `order` map
	// (`domain-model-v1.md`: "Navigator...导航排序"), committed through the same
	// `FlowObjectRepository`/`ObjectSession` pair a Page uses -- there is no separate write path.
	//
	// a11y (`contracts/ui-surface-v1.md` "a11y v0.4 基线", task brief item 4): `role=tree` with
	// roving tabindex, Arrow navigation/Home/End/Enter for the base tree, and a SEPARATE
	// "lift/move/drop" mode for reorder: Space lifts, ArrowUp/Down swaps sibling position live
	// (visually only, nothing is written to the CRDT until Drop), Escape cancels and reverts to
	// the last committed order with zero writes, a `Space` while lifted commits (Drop). Every
	// transition is announced through one `aria-live=polite` region.
	//
	// Same-parent reorder stays on the navigator order map. A parent change is `move_object`
	// (`ADR-0012` §4): the context menu and a drop onto a different parent send it. ArrowLeft
	// and ArrowRight while lifted stay sibling-only and announce that they cannot change parent.
	// Outside lift mode they are the ordinary treeitem collapse and expand gesture.

	import { getContext, onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { t } from 'svelte-i18n';
	import Modal from '#lib/components/Modal.svelte';
	import { flowApi, type FlowObjectView, type FlowTrashItem } from '#lib/api/flow.js';
	import { FlowCommandService } from '#lib/flow/command-service.js';
	import {
		getNamedMap,
		type FlowObjectRepository,
		type OpenFlowObjectResult
	} from '#lib/flow/object-repository.js';
	import {
		appendOrderKey,
		applyDestination,
		contextMenuDestination,
		keyboardStepDestination,
		orderKeyForDestination,
		pointerDropDestination,
		seedOrderKeys,
		type DropSide,
		type OrderedSibling
	} from '#lib/flow/navigator-reorder.js';
	import { FLOW_REPOSITORY_CONTEXT, FLOW_NAV_VIEW_CONTEXT } from '#lib/flow/context-keys.js';
	import {
		descendantIds,
		moveInsideTargets,
		moveObjectCommand,
		moveOutsideTarget,
		personalMoveAllowed
	} from '#lib/flow/cross-parent-move.js';
	import { flowErrorDetailKey } from '#lib/flow/errors.js';
	import type { FlowError } from '#lib/flow/types.js';
	import { resolveRenderer } from '#lib/flow/renderer-registry.js';
	import {
		FlowNavigatorSearch,
		SEARCH_QUERY_MAX_CHARS,
		type SearchSnapshot
	} from '#lib/flow/navigator-search.js';
	import { FlowLagBadge, type LagBadgeState } from '#lib/flow/navigator-lag.js';
	import type { AcceptedChange } from '#lib/api/flow.js';
	import CreateCollectionDialog from './CreateCollectionDialog.svelte';
	import FlowObjectIcon from './FlowObjectIcon.svelte';
	import { flowErrorFromResult } from '#lib/flow/command-service.js';

	// `contracts/ui-surface-v1.md` "Renderer registry": an unregistered `${objectType}:${viewType}`
	// must fail to read-only/unsupported rather than the route always assuming a tree renders.
	const navRenderer = resolveRenderer('navigator', 'tree');

	interface Props {
		scopeId: string;
		integrationWorkspaceId?: string;
		selectedObjectId: string | null;
	}

	let { scopeId, selectedObjectId, integrationWorkspaceId }: Props = $props();

	let placements = $state<import('#lib/flow/navigator-placements.js').NavigatorPlacement[]>([]);
	let refreshingNavigator = $state(false);
	let navigatorFailureKey = $state<string | null>(null);
	let confirmingPlacement = $state<string | null>(null);
	let removingPlacement = $state(false);
	async function refreshNavigator(): Promise<void> {
		if (refreshingNavigator) return;
		refreshingNavigator = true;
		try {
			await loadNavigatorRoot();
			await loadObjects();
			navigatorFailureKey = null;
		} catch (error) {
			navigatorFailureKey = flowErrorDetailKey(error as FlowError);
		} finally {
			refreshingNavigator = false;
		}
	}
	async function removePlacement(objectId: string, mountId: string): Promise<void> {
		if (removingPlacement) return;
		removingPlacement = true;
		const result = await flowApi.removeMount(objectId, mountId);
		if (result.code === 0 && result.data?.removed) {
			placements = placements.filter((item) => item.mountId !== mountId);
			confirmingPlacement = null;
		} else navigatorFailureKey = flowErrorDetailKey(flowErrorFromResult(result));
		removingPlacement = false;
	}

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commandService = new FlowCommandService();

	const navView = getContext<{ refresh: () => Promise<void> } | undefined>(FLOW_NAV_VIEW_CONTEXT);
	let navSnapshotKey = '';
	let navSnapshotUser: string | null = null;
	const taskWorkbench = getContext<TaskWorkbench | undefined>(WORKBENCH_CONTEXT);
	const flowTasks = getContext<(() => FlowTaskSessions | null) | undefined>(FLOW_TASK_SESSIONS)?.();
	let objects = $state<FlowObjectView[]>([]);
	let rootObjectId = $state<string | null>(null);
	let navEntry: OpenFlowObjectResult | null = null;
	let orderVersion = $state(0); // bumped whenever the order map changes, to re-derive the tree
	let loading = $state(true);
	let expanded = $state(new Set<string>());
	let focusedId = $state<string | null>(null);
	let liftedId = $state<string | null>(null);
	let draftSiblingOrder = $state<string[]>([]); // object ids, lift-mode only, visual-only until drop
	let announcement = $state('');
	let creatingParentId = $state<string | null | undefined>(undefined);

	// v0.5 Flow search (`ui-surface-v1.md` "后续版本 UI 派生" v0.5) and the v0.6 standalone
	// Collection create; debounce/abort/keyboard state lives in `#lib/flow/navigator-search.js`, the
	// lag badge's refresh loop in `#lib/flow/navigator-lag.js`.
	const search = new FlowNavigatorSearch(
		() => scopeId,
		(snapshot) => (searchState = snapshot),
		{ reads: repository }
	);
	let searchState = $state<SearchSnapshot>(search.snapshot);
	let searchInput = $state('');
	const searchListId = 'flow-search-results';
	const searchOptionId = (index: number): string => `${searchListId}-option-${index}`;
	const searchOpen = $derived(searchState.status !== 'idle');

	const lagBadge = new FlowLagBadge(
		() => scopeId,
		(state) => (lagState = state),
		{
			reads: {
				getProjectionLag: (scope) =>
					integrationWorkspaceId
						? repository.getWorkspaceProjectionLag(integrationWorkspaceId, { limit: 1 })
						: repository.getProjectionLag(scope)
			}
		}
	);
	let lagState = $state<LagBadgeState>(lagBadge.current);

	let newMenuOpen = $state(false);
	let newMenuButton = $state<HTMLButtonElement | null>(null);
	let newMenuEl = $state<HTMLDivElement | null>(null);
	let collectionDialogOpen = $state(false);
	let trashOpen = $state(false);
	let trashItems = $state<FlowTrashItem[]>([]);
	let trashFailureKey = $state<string | null>(null);
	let trashBusy = $state(false);
	let trashLoading = $state(false);
	let trashNextCursor = $state<string | null>(null);
	let trashRequest = 0;
	let confirmingDelete = $state<string | null>(null);

	const isNavigatorTrashItem = (item: FlowTrashItem) =>
		item.object_type === 'page' || item.object_type === 'collection';

	async function loadTrash(cursor: string | null = null): Promise<void> {
		const request = ++trashRequest;
		trashLoading = true;
		if (!cursor) trashNextCursor = null;
		const query = { limit: 50, ...(cursor ? { cursor } : {}) };
		const result = await (integrationWorkspaceId
			? flowApi.listWorkspaceTrash(integrationWorkspaceId, query)
			: flowApi.listTrash(scopeId, query));
		if (request !== trashRequest) return;
		trashLoading = false;
		if (result.code === 0 && result.data) {
			trashItems = cursor
				? [
						...new Map(
							[...trashItems, ...result.data.items.filter(isNavigatorTrashItem)].map((item) => [
								item.id,
								item
							])
						).values()
					]
				: result.data.items.filter(isNavigatorTrashItem);
			trashNextCursor = result.data.next_cursor ?? null;
			trashFailureKey = null;
		} else {
			trashFailureKey = flowErrorDetailKey(flowErrorFromResult(result));
		}
	}

	async function archiveObject(objectId: string): Promise<void> {
		closeContextMenu();
		try {
			await commandService.execute(objectId, { type: 'archive', payload: {} });
			await loadObjects();
			if (trashOpen) await loadTrash();
			announce('flow.trash.archived');
		} catch (error) {
			trashFailureKey = flowErrorDetailKey(error as FlowError);
		}
	}

	async function restoreArchived(objectId: string): Promise<void> {
		if (trashBusy) return;
		trashBusy = true;
		try {
			await commandService.execute(objectId, { type: 'restore', payload: {} });
			await loadObjects();
			await loadTrash();
		} catch (error) {
			trashFailureKey = flowErrorDetailKey(error as FlowError);
		} finally {
			trashBusy = false;
		}
	}

	async function purgeArchived(objectId: string): Promise<void> {
		if (trashBusy) return;
		trashBusy = true;
		const result = await flowApi.purgeObject(objectId);
		trashBusy = false;
		confirmingDelete = null;
		if (result.code !== 0) {
			const details = result.details;
			const reason =
				typeof details === 'object' && details !== null && 'reason' in details
					? (details as { reason?: unknown }).reason
					: null;
			trashFailureKey =
				reason === 'active_descendant'
					? 'flow.trash.activeDescendant'
					: flowErrorDetailKey(flowErrorFromResult(result));
			return;
		}
		await loadTrash();
	}

	function orderOf(objectId: string): string {
		if (!navEntry) return '';
		const value = getNamedMap(navEntry.doc, 'order').get(objectId);
		return typeof value === 'string' ? value : '';
	}

	interface TreeNode {
		object: FlowObjectView;
		children: TreeNode[];
		depth: number;
	}

	const tree = $derived.by((): TreeNode[] => {
		void orderVersion;
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local scratch collection, not reactive state
		const byParent = new Map<string | null, FlowObjectView[]>();
		for (const object of objects) {
			const parentKey = object.parent_id ?? null;
			const siblings = byParent.get(parentKey) ?? [];
			siblings.push(object);
			byParent.set(parentKey, siblings);
		}
		for (const siblings of byParent.values()) {
			siblings.sort(
				(a, b) =>
					orderOf(a.id).localeCompare(orderOf(b.id)) || a.created_at.localeCompare(b.created_at)
			);
		}
		function build(parentId: string | null, depth: number): TreeNode[] {
			const siblings = byParent.get(parentId) ?? [];
			return siblings.map((object) => ({
				object,
				depth,
				children: expanded.has(object.id) ? build(object.id, depth + 1) : []
			}));
		}
		return rootObjectId ? build(rootObjectId, 0) : [];
	});

	function flattenVisible(nodes: TreeNode[] = tree): TreeNode[] {
		const out: TreeNode[] = [];
		for (const node of nodes) {
			out.push(node);
			if (node.children.length > 0) out.push(...flattenVisible(node.children));
		}
		return out;
	}

	function siblingsOf(objectId: string): FlowObjectView[] {
		const object = objects.find((o) => o.id === objectId);
		const parentId = object?.parent_id ?? null;
		return objects
			.filter((o) => (o.parent_id ?? null) === parentId)
			.sort((a, b) => orderOf(a.id).localeCompare(orderOf(b.id)));
	}

	/** The same sibling list in the shape `navigator-reorder` works in: committed order, each
	 * entry carrying its current fractional-index key. Every gesture below builds its input with
	 * this one function, so no gesture can be reasoning about a differently-sorted list. */
	function orderedSiblingsOf(objectId: string): OrderedSibling[] {
		return siblingsOf(objectId).map((o) => ({ id: o.id, orderKey: orderOf(o.id) }));
	}

	/** Writes one reorder. The destination always comes from `navigator-reorder`; this function
	 * owns the CRDT write and the announcement, and never computes bounds itself.
	 *
	 * `destination === null` means the gesture could not express a move at all (end of list,
	 * cross-parent drop, unknown id) -- that is the announced failure. A destination that resolves
	 * to no key change means the object is already there: nothing is written and nothing is
	 * announced as a failure, because landing where you started is a no-op, not an error. */
	function commitReorder(objectId: string, destination: number | null): boolean {
		if (!navEntry || destination === null || !canReorderPage(objectId)) {
			announce('flow.nav.liveRegion.reorderFailed');
			return false;
		}
		const newKey = orderKeyForDestination(orderedSiblingsOf(objectId), objectId, destination);
		if (newKey === null) return false;
		try {
			getNamedMap(navEntry.doc, 'order').set(objectId, newKey);
			navEntry.doc.commit();
			orderVersion += 1;
		} catch {
			announce('flow.nav.liveRegion.reorderFailed');
			return false;
		}
		const object = objects.find((o) => o.id === objectId);
		announce('flow.nav.liveRegion.dropped', { title: object?.title ?? '' });
		return true;
	}

	async function ensureOrderEntries(): Promise<void> {
		if (!navEntry) return;
		const map = getNamedMap(navEntry.doc, 'order');
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local scratch collection, not reactive state
		const byParent = new Map<string | null, FlowObjectView[]>();
		for (const object of objects) {
			const parentKey = object.parent_id ?? null;
			const siblings = byParent.get(parentKey) ?? [];
			siblings.push(object);
			byParent.set(parentKey, siblings);
		}
		let wrote = false;
		for (const siblings of byParent.values()) {
			const missing = siblings.filter((o) => typeof map.get(o.id) !== 'string');
			if (missing.length === 0) continue;
			const existingKeys = siblings
				.map((o) => map.get(o.id))
				.filter((v): v is string => typeof v === 'string');
			const ordered = missing.sort((a, b) => a.created_at.localeCompare(b.created_at));
			const keys = seedOrderKeys(existingKeys, ordered.length);
			ordered.forEach((object, index) => {
				map.set(object.id, keys[index]);
				wrote = true;
			});
		}
		if (wrote) {
			navEntry.doc.commit();
			orderVersion += 1;
		}
	}

	/** Pages and collections make the tree; the records of a collection are read inside it. */
	const NAVIGATOR_OBJECT_TYPES = ['page', 'collection'] as const;

	async function loadObjectsOfType(
		objectType: (typeof NAVIGATOR_OBJECT_TYPES)[number]
	): Promise<FlowObjectView[]> {
		let cursor: string | undefined;
		const loaded: FlowObjectView[] = [];
		for (let pageIndex = 0; pageIndex < 10; pageIndex += 1) {
			const result = await commandService.listObjects(scopeId, {
				limit: 100,
				object_type: objectType,
				...(cursor ? { cursor } : {})
			});
			if (result.code !== 0 || !result.data) {
				navigatorFailureKey = flowErrorDetailKey(flowErrorFromResult(result));
				throw flowErrorFromResult(result);
			}
			loaded.push(
				...result.data.items.filter(
					(object) => object.scope_id === scopeId && object.object_type === objectType
				)
			);
			cursor = result.data.next_cursor ?? undefined;
			if (!cursor) return loaded;
		}
		throw {
			code: 'limit_exceeded',
			recoverable: false,
			details: { limit_kind: 'scan_budget', limit: 1000 }
		};
	}

	async function loadObjects(): Promise<void> {
		const loaded: FlowObjectView[] = [];
		for (const objectType of NAVIGATOR_OBJECT_TYPES)
			loaded.push(...(await loadObjectsOfType(objectType)));
		objects = loaded;
	}

	async function loadNavigatorRoot(): Promise<string> {
		const result = await commandService.getNavigator(scopeId);
		if (result.code !== 0 || !result.data) {
			throw flowErrorFromResult(result);
		}
		placements = navigatorPlacements(result.data.nodes.filter((node) => node.type === 'page'));
		rootObjectId = result.data.root_object_id;
		return result.data.root_object_id;
	}

	onMount(() => {
		const taskKey = taskWorkbench?.current.active;
		const controller = new AbortController();
		let unsubscribeNavigator = () => {};
		let refreshing = false;
		let refreshAgain = false;
		async function refreshNavigatorMetadata(): Promise<void> {
			refreshAgain = true;
			if (refreshing) return;
			refreshing = true;
			try {
				while (refreshAgain && !controller.signal.aborted) {
					refreshAgain = false;
					await loadNavigatorRoot();
					await loadObjects();
				}
			} catch (error) {
				if (!controller.signal.aborted)
					navigatorFailureKey = flowErrorDetailKey(error as FlowError);
			} finally {
				refreshing = false;
			}
		}
		if (navView) navView.refresh = refreshNavigatorMetadata;
		const saved = navSnapshotKey
			? taskWorkbench?.snapshot<{
					expanded: string[];
					search: string;
					focused: string | null;
					trash: boolean;
				}>(navSnapshotKey)
			: undefined;
		if (saved) {
			expanded = new Set(saved.expanded);
			searchInput = saved.search;
			focusedId = saved.focused;
			trashOpen = saved.trash;
			if (saved.search) search.setQuery(saved.search);
		}
		void (async () => {
			try {
				const navigatorObjectId = await loadNavigatorRoot();
				await loadObjects();
				// The tree needs only the REST reads above (created_at order until the order map
				// arrives). Waiting for the collaborative navigator document to connect would hide
				// every Page for as long as that socket is slow.
				loading = false;
				navEntry = await repository.open({
					scopeId,
					objectId: navigatorObjectId,
					signal: controller.signal
				});
				if (controller.signal.aborted) {
					await repository.close(navigatorObjectId);
					return;
				}
				if (taskKey) await flowTasks?.retain(taskKey, 'navigator', scopeId, navigatorObjectId);
				if (controller.signal.aborted) return;
				unsubscribeNavigator = observeNavigatorDocument(
					navEntry,
					() => {
						orderVersion += 1;
					},
					() => {
						void refreshNavigatorMetadata();
					}
				);
				// The stored order is now readable; re-derive the tree even if nothing needs seeding.
				orderVersion += 1;
				await ensureOrderEntries();
			} catch (error) {
				navigatorFailureKey = flowErrorDetailKey(error as FlowError);
			} finally {
				loading = false;
			}
		})();
		lagBadge.start();
		// A Page's slash-menu Collection embed creates a child of that Page outside this
		// component; re-read the list so it shows up under its (expanded) parent.
		const unsubscribeCreated = repository.createdObjects.subscribe((created) => {
			if (!created) return;
			void (async () => {
				await loadObjects();
				await ensureOrderEntries();
				if (created.parentId) {
					// eslint-disable-next-line svelte/prefer-svelte-reactivity -- replaced whole, like every other `expanded` write here
					expanded = new Set(expanded).add(created.parentId);
				}
			})().catch((error) => {
				navigatorFailureKey = flowErrorDetailKey(error as FlowError);
			});
		});
		return () => {
			controller.abort();
			unsubscribeNavigator();
			lagBadge.stop();
			search.dispose();
			unsubscribeCreated();
		};
	});

	onDestroy(() => {
		if (navSnapshotKey && taskWorkbench?.current.userId === navSnapshotUser)
			taskWorkbench.saveSnapshot(navSnapshotKey, {
				expanded: [...expanded],
				search: searchInput,
				focused: focusedId,
				trash: trashOpen
			});
		if (navView) navView.refresh = async () => {};
		if (navEntry) void repository.close(navEntry.handle.objectId);
	});

	function onSearchInput(): void {
		search.setQuery(searchInput);
	}

	function openSearchRow(index: number): void {
		const row = searchState.rows[index];
		if (!row) return;
		searchInput = '';
		search.clear();
		select(row.id);
	}

	function onSearchKeydown(event: KeyboardEvent): void {
		if (event.key === 'Escape') {
			if (searchInput === '') return;
			event.preventDefault();
			searchInput = '';
			search.clear();
			return;
		}
		if (event.key === 'Enter') {
			if (searchState.activeIndex < 0) return;
			event.preventDefault();
			openSearchRow(searchState.activeIndex);
			return;
		}
		if (!searchOpen) return;
		if (search.handleKey(event.key)) event.preventDefault();
	}

	function onFreshnessChange(event: Event): void {
		const checked = (event.currentTarget as HTMLInputElement).checked;
		search.setFreshness(checked ? 'require_current' : 'allow_stale');
	}

	function newMenuItems(): HTMLElement[] {
		return newMenuEl ? [...newMenuEl.querySelectorAll<HTMLElement>('[role="menuitem"]')] : [];
	}

	function toggleNewMenu(): void {
		newMenuOpen = !newMenuOpen;
		if (newMenuOpen) queueMicrotask(() => newMenuItems()[0]?.focus());
	}

	function closeNewMenu(returnFocus: boolean): void {
		newMenuOpen = false;
		if (returnFocus) newMenuButton?.focus();
	}

	function onNewMenuKeydown(event: KeyboardEvent): void {
		const items = newMenuItems();
		const index = items.indexOf(document.activeElement as HTMLElement);
		switch (event.key) {
			case 'Escape':
				event.preventDefault();
				closeNewMenu(true);
				return;
			case 'ArrowDown':
				event.preventDefault();
				items[Math.min(index + 1, items.length - 1)]?.focus();
				return;
			case 'ArrowUp':
				event.preventDefault();
				items[Math.max(index - 1, 0)]?.focus();
				return;
			case 'Home':
				event.preventDefault();
				items[0]?.focus();
				return;
			case 'End':
				event.preventDefault();
				items.at(-1)?.focus();
				return;
			case 'Tab':
				closeNewMenu(false);
				return;
			default:
				return;
		}
	}

	function chooseNewPage(): void {
		closeNewMenu(false);
		void createPage();
	}

	function chooseNewCollection(): void {
		closeNewMenu(false);
		collectionDialogOpen = true;
	}

	function onCollectionCreated(change: AcceptedChange): void {
		collectionDialogOpen = false;
		placeCreatedObject(change);
		select(change.object.id);
	}

	function announce(key: string, values?: Record<string, unknown>): void {
		announcement = get(t)(key, { values });
	}

	function toggleExpand(id: string): void {
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- expanded is replaced as a whole, so a plain Set is intended
		const next = new Set(expanded);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		expanded = next;
	}

	function select(id: string): void {
		void goto(
			integrationWorkspaceId
				? resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
						workspaceId: integrationWorkspaceId,
						objectId: id
					})
				: resolve('/(app)/flow/[objectId]', { objectId: id })
		);
	}

	function onTreeKeydown(event: KeyboardEvent, node: TreeNode): void {
		const visible = flattenVisible();
		const index = visible.findIndex((n) => n.object.id === node.object.id);

		if (liftedId === node.object.id) {
			switch (event.key) {
				case 'ArrowUp':
				case 'ArrowDown': {
					event.preventDefault();
					const delta = event.key === 'ArrowUp' ? -1 : 1;
					// The draft is a preview of the SAME destination the drop will commit -- both go
					// through `navigator-reorder`, so what the live region announces during a lift
					// cannot drift from what lands in the CRDT on Drop.
					const draft = draftSiblingOrder.map((id) => ({ id, orderKey: orderOf(id) }));
					const destination = keyboardStepDestination(draft, node.object.id, delta);
					if (destination === null) return;
					draftSiblingOrder = applyDestination(draftSiblingOrder, node.object.id, destination);
					announce('flow.nav.liveRegion.moved', {
						title: node.object.title,
						position: destination + 1
					});
					return;
				}
				case 'ArrowLeft':
				case 'ArrowRight':
					event.preventDefault();
					announce('flow.nav.liveRegion.reorderFailed');
					return;
				case ' ':
				case 'Enter':
					event.preventDefault();
					void commitLift(node.object.id);
					return;
				case 'Escape':
					event.preventDefault();
					cancelLift(node.object.title);
					return;
				default:
					return;
			}
		}

		switch (event.key) {
			case 'ContextMenu': {
				event.preventDefault();
				const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
				openContextMenu(node.object.id, rect.left, rect.bottom);
				return;
			}
			case 'F10': {
				if (!event.shiftKey) return;
				event.preventDefault();
				const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
				openContextMenu(node.object.id, rect.left, rect.bottom);
				return;
			}
			case 'ArrowDown': {
				event.preventDefault();
				const next = visible[index + 1];
				if (next) focusedId = next.object.id;
				return;
			}
			case 'ArrowUp': {
				event.preventDefault();
				const previous = visible[index - 1];
				if (previous) focusedId = previous.object.id;
				return;
			}
			case 'ArrowRight':
				event.preventDefault();
				if (!expanded.has(node.object.id)) toggleExpand(node.object.id);
				return;
			case 'ArrowLeft':
				event.preventDefault();
				if (expanded.has(node.object.id)) toggleExpand(node.object.id);
				return;
			case 'Home':
				event.preventDefault();
				if (visible[0]) focusedId = visible[0].object.id;
				return;
			case 'End':
				event.preventDefault();
				if (visible.at(-1)) focusedId = visible.at(-1)!.object.id;
				return;
			case 'Enter':
				event.preventDefault();
				select(node.object.id);
				return;
			case ' ':
				event.preventDefault();
				startLift(node.object.id);
				return;
			default:
				return;
		}
	}

	function canReorderPage(objectId: string): boolean {
		const object = objects.find((item) => item.id === objectId);
		return (
			!!object &&
			(object.parent_id === rootObjectId ||
				objects.some((parent) => parent.id === object.parent_id))
		);
	}
	function startLift(objectId: string): void {
		if (!canReorderPage(objectId)) {
			announce('flow.nav.liveRegion.moveFailed');
			return;
		}
		liftedId = objectId;
		draftSiblingOrder = siblingsOf(objectId).map((o) => o.id);
		const object = objects.find((o) => o.id === objectId);
		announce('flow.nav.liveRegion.lifted', { title: object?.title ?? '' });
	}

	function cancelLift(title: string): void {
		liftedId = null;
		draftSiblingOrder = [];
		announce('flow.nav.liveRegion.cancelled', { title });
	}

	async function commitLift(objectId: string): Promise<void> {
		// The draft already holds the object at its intended position, and `applyDestination` puts
		// it at exactly index `destination` -- so the draft index IS the destination, in the same
		// coordinate system every other gesture uses.
		const index = draftSiblingOrder.indexOf(objectId);
		commitReorder(objectId, index < 0 ? null : index);
		liftedId = null;
		draftSiblingOrder = [];
	}

	// Context menu (`contracts/ui-surface-v1.md` a11y baseline: "context menu 提供 Move before/
	// after/inside/outside，功能与 pointer drag 相同"). "before"/"after" reuse the exact
	// same-parent CRDT order write the lift/drop and pointer-drag paths already use above -- there
	// is still only one write path for a reorder. "inside"/"outside" would nest under a different
	// parent, which -- matching the same documented v0.4 scope note at the top of this file for
	// ArrowLeft/ArrowRight while lifted -- is a governance `move_object` command that does not
	// exist until v0.5, so those two items announce "not available this version" and touch no
	// state, exactly like the keyboard path already does.
	let contextMenuFor = $state<string | null>(null);
	let contextMenuPos = $state<{ x: number; y: number } | null>(null);

	function openContextMenu(objectId: string, x: number, y: number): void {
		contextMenuFor = objectId;
		contextMenuPos = { x, y };
	}

	function closeContextMenu(): void {
		contextMenuFor = null;
		contextMenuPos = null;
	}

	function onTreeContextMenu(event: MouseEvent, objectId: string): void {
		event.preventDefault();
		openContextMenu(objectId, event.clientX, event.clientY);
	}

	async function moveRelative(objectId: string, direction: DropSide): Promise<void> {
		closeContextMenu();
		commitReorder(
			objectId,
			contextMenuDestination(orderedSiblingsOf(objectId), objectId, direction)
		);
	}

	function moveNodes(): { id: string; parent_id: string | null }[] {
		return objects.map((object) => ({ id: object.id, parent_id: object.parent_id ?? null }));
	}

	async function moveToParent(objectId: string, targetObjectId: string): Promise<void> {
		const source = objects.find((object) => object.id === objectId);
		const targetType =
			targetObjectId === navEntry?.handle.objectId
				? 'navigator'
				: objects.find((object) => object.id === targetObjectId)?.object_type;
		if (!integrationWorkspaceId && !personalMoveAllowed(source?.object_type, targetType)) {
			announce('flow.nav.liveRegion.moveFailed');
			return;
		}
		try {
			await commandService.execute(objectId, moveObjectCommand(targetObjectId));
			await loadObjects();
			const object = objects.find((candidate) => candidate.id === objectId);
			announce('flow.nav.liveRegion.movedUnder', { title: object?.title ?? '' });
		} catch (error) {
			announce(flowErrorDetailKey(error as FlowError));
		}
	}

	let moveInsideFor = $state<string | null>(null);

	function openMoveInside(objectId: string): void {
		closeContextMenu();
		moveInsideFor = objectId;
	}

	function moveOutside(objectId: string): void {
		closeContextMenu();
		const target = moveOutsideTarget(moveNodes(), objectId);
		if (!target) {
			announce('flow.nav.liveRegion.moveFailed');
			return;
		}
		void moveToParent(objectId, target);
	}

	const insideChoices = $derived.by(() => {
		if (!moveInsideFor) return [];
		return moveInsideTargets(moveNodes(), moveInsideFor).flatMap((id) => {
			const object = objects.find((candidate) => candidate.id === id);
			const source = objects.find((candidate) => candidate.id === moveInsideFor);
			return object &&
				(integrationWorkspaceId || personalMoveAllowed(source?.object_type, object.object_type))
				? [{ id, title: object.title }]
				: [];
		});
	});

	// Pointer drag: same-parent-only, matching the keyboard equivalent's scope exactly.
	let dragSourceId: string | null = null;

	function onDragStart(objectId: string): void {
		dragSourceId = objectId;
	}

	function onDragOver(event: DragEvent, targetId: string): void {
		if (!dragSourceId || dragSourceId === targetId) return;
		const source = objects.find((o) => o.id === dragSourceId);
		const target = objects.find((o) => o.id === targetId);
		if (!source || !target) return;
		if (descendantIds(moveNodes(), source.id).has(target.id)) return;
		event.preventDefault();
	}

	async function onDrop(event: DragEvent, targetId: string): Promise<void> {
		event.preventDefault();
		const sourceId = dragSourceId;
		dragSourceId = null;
		if (!sourceId || !navEntry) return;
		const source = objects.find((o) => o.id === sourceId);
		const target = objects.find((o) => o.id === targetId);
		if (!source || !target || sourceId === targetId) return;
		if ((source.parent_id ?? null) !== (target.parent_id ?? null)) {
			if (descendantIds(moveNodes(), source.id).has(target.id)) {
				announce('flow.nav.liveRegion.moveFailed');
				return;
			}
			await moveToParent(source.id, target.id);
			return;
		}

		// Pointer drag resolves to the same destination-then-key pipeline the keyboard and context
		// menu use, and produces the same live-region announcement: the a11y baseline treats them
		// as one operation with three input methods, not three operations.
		const siblings = orderedSiblingsOf(source.id);
		commitReorder(source.id, pointerDropDestination(siblings, source.id, targetId));
	}

	async function createPage(parentId?: string): Promise<void> {
		if (creatingParentId !== undefined) return;
		creatingParentId = parentId ?? null;
		try {
			const created = await commandService.createObject(scopeId, {
				object_type: 'page',
				parent_object_id: parentId,
				title: get(t)('flow.nav.untitled')
			});
			placeCreatedObject(created);
			// eslint-disable-next-line svelte/prefer-svelte-reactivity -- expanded is replaced as a whole, so a plain Set is intended
			if (parentId) expanded = new Set(expanded).add(parentId);
			select(created.object.id);
		} catch (error) {
			// Typed rejection: the key comes from the stable code and its named reason
			// (`child_project_must_match_parent` tells the user to use the parent's project),
			// never from the message.
			announce(flowErrorDetailKey(error as FlowError));
		} finally {
			creatingParentId = undefined;
		}
	}

	/** Adds a just-created object to the tree and appends it to its siblings' order. The one
	 * create-side order write, shared by Page and Collection create. */
	function placeCreatedObject(created: AcceptedChange): void {
		objects = [...objects.filter((o) => o.id !== created.object.id), created.object];
		if (navEntry) {
			const siblings = orderedSiblingsOf(created.object.id).filter(
				(o) => o.id !== created.object.id
			);
			getNamedMap(navEntry.doc, 'order').set(created.object.id, appendOrderKey(siblings));
			navEntry.doc.commit();
			orderVersion += 1;
		}
	}
</script>

<nav
	class="flex h-full w-full shrink-0 flex-col border-r border-line bg-nav"
	aria-label={$t('flow.nav.title')}
>
	<a
		href={integrationWorkspaceId
			? resolve('/(app)/workspace/[workspaceId]/settings/flow/package', {
					workspaceId: integrationWorkspaceId
				})
			: resolve('/(app)/flow/package')}
		class="block min-h-11 px-3 py-2 text-sm text-fg-muted underline"
		>{$t('flow.personal.packages')}</a
	>
	<div class="flex items-center justify-between border-b border-line p-3">
		<span class="text-sm font-semibold text-fg">{$t('flow.nav.title')}</span>
		<div class="relative flex items-center gap-1">
			<button
				type="button"
				class="min-h-11 min-w-11 rounded px-2 text-fg-muted hover:bg-selected"
				aria-label={$t('flow.personal.refreshNavigator')}
				title={$t('flow.personal.refreshNavigator')}
				disabled={refreshingNavigator}
				aria-busy={refreshingNavigator}
				onclick={refreshNavigator}>↻</button
			>
			<button
				type="button"
				bind:this={newMenuButton}
				class="flex min-h-11 min-w-11 items-center justify-center rounded text-fg-muted hover:bg-selected hover:text-fg disabled:text-fg-muted disabled:hover:bg-transparent"
				onclick={toggleNewMenu}
				disabled={creatingParentId !== undefined}
				aria-haspopup="menu"
				aria-expanded={newMenuOpen}
				aria-label={$t('flow.nav.newMenu')}
				title={$t('flow.nav.newMenu')}
				data-testid="flow-nav-new"
			>
				<svg
					class="h-5 w-5"
					fill="none"
					stroke="currentColor"
					viewBox="0 0 24 24"
					aria-hidden="true"
				>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M12 5v14M5 12h14"
					/>
				</svg>
			</button>
			{#if newMenuOpen}
				<div
					class="fixed inset-0 z-40"
					role="presentation"
					onclick={() => closeNewMenu(false)}
				></div>
				<div
					bind:this={newMenuEl}
					class="absolute right-0 top-full z-50 mt-1 w-48 rounded-float border border-overlay-line bg-overlay py-1 text-sm shadow-overlay"
					role="menu"
					tabindex="-1"
					aria-label={$t('flow.nav.newMenu')}
					onkeydown={onNewMenuKeydown}
				>
					<button
						type="button"
						role="menuitem"
						class="block min-h-11 w-full px-3 text-left text-fg hover:bg-surface focus:bg-surface"
						onclick={chooseNewPage}
					>
						{$t('flow.nav.newPage')}
					</button>
					<button
						type="button"
						role="menuitem"
						class="block min-h-11 w-full px-3 text-left text-fg hover:bg-surface focus:bg-surface"
						onclick={chooseNewCollection}
					>
						{$t('flow.nav.createCollection.menuItem')}
					</button>
				</div>
			{/if}
		</div>
	</div>

	<p
		class="border-b border-line px-3 py-1 text-xs {lagState.kind === 'lagging'
			? 'text-warning'
			: 'text-fg-muted'}"
		role="status"
		aria-live="polite"
		data-testid="flow-lag-badge"
		data-lag-state={lagState.kind}
	>
		{#if lagState.kind === 'loading'}
			{$t('flow.lag.loading')}
		{:else if lagState.kind === 'synced'}
			{$t('flow.lag.synced')}
		{:else if lagState.kind === 'lagging'}
			{$t('flow.lag.lagging', { values: { max: lagState.maxLag, p95: lagState.p95Lag } })}
		{:else}
			{$t('flow.lag.unavailable')}
		{/if}
	</p>

	<div class="border-b border-line p-2">
		<input
			type="search"
			role="combobox"
			bind:value={searchInput}
			oninput={onSearchInput}
			onkeydown={onSearchKeydown}
			maxlength={SEARCH_QUERY_MAX_CHARS}
			placeholder={$t('flow.search.placeholder')}
			aria-label={$t('flow.search.label')}
			aria-autocomplete="list"
			aria-expanded={searchOpen}
			aria-controls={searchListId}
			aria-activedescendant={searchOpen && searchState.activeIndex >= 0
				? searchOptionId(searchState.activeIndex)
				: undefined}
			class="min-h-11 w-full rounded-control border border-input bg-field px-3 text-sm text-fg"
			data-testid="flow-search-input"
		/>
		{#if searchOpen}
			<label class="mt-1 flex min-h-11 items-center gap-2 text-xs text-fg-muted">
				<input
					type="checkbox"
					checked={searchState.freshness === 'require_current'}
					onchange={onFreshnessChange}
					data-testid="flow-search-require-current"
				/>
				<span>{$t('flow.search.requireCurrent')}</span>
			</label>
		{/if}
	</div>

	<div class="flex-1 overflow-y-auto p-2">
		{#if searchOpen}
			<div data-testid="flow-search-panel">
				{#if searchState.status === 'pending' || searchState.status === 'loading'}
					<p class="p-2 text-sm text-fg-muted">
						{$t('flow.search.searching')}
					</p>
				{:else if searchState.status === 'error' && searchState.failureKey}
					<p
						class="p-2 text-sm text-danger"
						role="alert"
						data-testid="flow-search-error"
						data-error-code={searchState.failureCode ?? ''}
					>
						{$t(searchState.failureKey)}
					</p>
				{:else if searchState.status === 'ready' && searchState.rows.length === 0}
					<p class="p-2 text-sm text-fg-muted" data-testid="flow-search-empty">
						{$t('flow.search.empty')}
					</p>
				{/if}
				{#if searchState.status === 'ready' && searchState.frontierStale}
					<p class="px-2 pb-1 text-xs text-warning" data-testid="flow-search-frontier-stale">
						{$t('flow.search.frontierStale', { values: { lag: searchState.frontierLag } })}
					</p>
				{/if}
				<ul
					id={searchListId}
					role="listbox"
					aria-label={$t('flow.search.resultsLabel')}
					class="space-y-1"
				>
					{#each searchState.rows as row, index (row.id)}
						<!-- Focus stays in the combobox input (aria-activedescendant); the option is
							 activated by pointer here and by Enter on the input. -->
						<!-- svelte-ignore a11y_click_events_have_key_events -->
						<li
							id={searchOptionId(index)}
							role="option"
							aria-selected={index === searchState.activeIndex}
							class={`min-h-11 cursor-pointer rounded px-2 py-1.5 text-sm ${
								index === searchState.activeIndex
									? 'bg-selected text-fg'
									: 'text-fg hover:bg-selected'
							}`}
							onclick={() => openSearchRow(index)}
							onmousemove={() => {
								if (index !== searchState.activeIndex) search.setActive(index);
							}}
							data-testid="flow-search-result"
							data-object-id={row.id}
						>
							<span class="flex items-center gap-2">
								<span class="flex-1 truncate font-medium"
									>{row.title || $t('flow.nav.untitled')}</span
								>
								<span class="shrink-0 text-xs text-fg-muted"
									>{$t(
										row.objectType === 'collection'
											? 'flow.search.type.collection'
											: row.objectType === 'navigator'
												? 'flow.search.type.navigator'
												: 'flow.search.type.page'
									)}</span
								>
								{#if row.stale}
									<span
										class="shrink-0 rounded bg-warning-soft px-1 text-xs text-warning"
										data-testid="flow-search-stale"
										title={$t('flow.search.staleHint', { values: { lag: row.lag } })}
									>
										{$t('flow.search.stale')}
									</span>
								{/if}
							</span>
							{#if row.snippet.length > 0}
								<span class="mt-0.5 block truncate text-xs text-fg-muted"
									>{#each row.snippet as segment, segmentIndex (segmentIndex)}{#if segment.match}<mark
												class="bg-warning-soft text-inherit">{segment.text}</mark
											>{:else}{segment.text}{/if}{/each}</span
								>
							{/if}
						</li>
					{/each}
				</ul>
				<p class="sr-only" aria-live="polite" role="status">
					{searchState.status === 'ready'
						? $t('flow.search.resultCount', { values: { count: searchState.rows.length } })
						: ''}
				</p>
			</div>
		{:else if !navRenderer}
			<p class="p-2 text-sm text-fg-muted">{$t('flow.canvas.unsupported')}</p>
		{:else if loading}
			<p class="p-2 text-sm text-fg-muted">{$t('flow.nav.loading')}</p>
		{:else if tree.length === 0}
			<div class="p-3 text-sm text-fg-muted">
				<p class="mb-2">{$t('flow.nav.empty')}</p>
				<button type="button" class="font-medium text-fg underline" onclick={() => createPage()}>
					{$t('flow.nav.createFirst')}
				</button>
			</div>
		{:else}
			{#snippet renderNode(node: TreeNode)}
				<div role="none">
					<div
						role="treeitem"
						tabindex={focusedId === node.object.id ||
						(focusedId === null && node.depth === 0 && tree[0] === node)
							? 0
							: -1}
						aria-level={node.depth + 1}
						aria-expanded={node.children.length > 0 || undefined}
						aria-selected={selectedObjectId === node.object.id}
						draggable={canReorderPage(node.object.id)}
						class={`group flex cursor-pointer items-center gap-1 rounded px-2 py-1.5 text-sm outline-none ${
							selectedObjectId === node.object.id
								? 'bg-selected text-fg'
								: 'text-fg hover:bg-selected'
						} ${liftedId === node.object.id ? 'ring-2 ring-warning' : ''} ${
							focusedId === node.object.id ? 'ring-2 ring-focus' : ''
						}`}
						style={`padding-left: ${node.depth * 16 + 8}px`}
						onclick={() => select(node.object.id)}
						onkeydown={(e) => onTreeKeydown(e, node)}
						onfocus={() => (focusedId = node.object.id)}
						oncontextmenu={(e) => onTreeContextMenu(e, node.object.id)}
						ondragstart={() => onDragStart(node.object.id)}
						ondragover={(e) => onDragOver(e, node.object.id)}
						ondrop={(e) => onDrop(e, node.object.id)}
					>
						{#if node.children.length > 0 || objects.some((o) => o.parent_id === node.object.id)}
							<button
								type="button"
								class="w-4 shrink-0 text-fg-muted"
								onclick={(e) => {
									e.stopPropagation();
									toggleExpand(node.object.id);
								}}
								aria-label={expanded.has(node.object.id)
									? $t('flow.nav.collapse')
									: $t('flow.nav.expand')}
							>
								{expanded.has(node.object.id) ? '▾' : '▸'}
							</button>
						{:else}
							<span class="w-4 shrink-0"></span>
						{/if}
						<FlowObjectIcon appearance={node.object.appearance} objectId={node.object.id} />
						<span class="truncate">{node.object.title || $t('flow.nav.untitled')}</span>
						{#if node.object.object_type === 'collection'}
							<span
								class="ml-auto shrink-0 text-xs text-fg-muted"
								data-testid="flow-nav-collection-badge">{$t('flow.nav.collectionBadge')}</span
							>
						{/if}
					</div>
					{#if expanded.has(node.object.id)}
						<div role="group">
							{#each node.children as child (child.object.id)}
								{@render renderNode(child)}
							{/each}
						</div>
					{/if}
				</div>
			{/snippet}

			<div role="tree" aria-label={$t('flow.nav.title')}>
				{#each tree as node (node.object.id)}
					{@render renderNode(node)}
				{/each}
			</div>
		{/if}
		{#if placements.length > 0}
			<section
				class="mt-3 border-t border-line pt-2"
				aria-labelledby="flow-placements-heading"
				data-testid="flow-nav-placements"
			>
				<h2 id="flow-placements-heading" class="px-2 text-xs font-semibold text-fg-muted">
					{$t('flow.mounts.sharedObjects')}
				</h2>
				<ul class="mt-1 space-y-1">
					{#each placements as placement (placement.mountId)}
						<li
							data-mount-id={placement.mountId}
							data-source-scope={placement.scopeId}
							class="rounded px-2 text-sm"
						>
							{#if placement.status === 'source_archived'}
								<p class="py-2 text-fg-muted">
									{placement.title || $t('flow.nav.untitled')} · {$t('flow.mounts.sourceArchived')}
								</p>
							{:else}
								<a
									href={integrationWorkspaceId
										? resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
												workspaceId: integrationWorkspaceId,
												objectId: placement.objectId
											})
										: resolve('/(app)/flow/[objectId]', { objectId: placement.objectId })}
									draggable={false}
									aria-current={selectedObjectId === placement.objectId ? 'page' : undefined}
									title={$t('flow.mounts.nonOwningHint')}
									class="flex min-h-11 items-center gap-2 rounded text-fg hover:underline"
								>
									<FlowObjectIcon objectId={placement.objectId} appearance={placement.appearance} />
									<span class="min-w-0 flex-1 truncate"
										>{placement.title || $t('flow.nav.untitled')}</span
									>
									<span class="shrink-0 text-xs text-fg-muted">{$t('flow.mounts.sharedBadge')}</span
									>
								</a>
							{/if}
							{#if placement.canRemove}
								{#if confirmingPlacement === placement.mountId}
									<p class="text-xs">{$t('flow.mounts.confirm')}</p>
									<button
										type="button"
										class="min-h-11 px-1 text-xs text-danger underline"
										disabled={removingPlacement}
										onclick={() => removePlacement(placement.objectId, placement.mountId)}
										>{$t('flow.mounts.remove')}</button
									>
									<button
										type="button"
										class="min-h-11 px-1 text-xs"
										disabled={removingPlacement}
										onclick={() => (confirmingPlacement = null)}>{$t('flow.mounts.cancel')}</button
									>
								{:else}
									<button
										type="button"
										class="min-h-11 text-xs text-fg-muted underline"
										onclick={() => (confirmingPlacement = placement.mountId)}
										>{$t('flow.mounts.remove')}</button
									>
								{/if}
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}
		{#if navigatorFailureKey}<p class="p-2 text-xs text-danger" role="alert">
				{$t(navigatorFailureKey)}
			</p>{/if}

		<div class="mt-3 border-t border-line pt-2" data-testid="flow-trash">
			<button
				type="button"
				class="flex min-h-11 w-full items-center px-2 text-left text-sm text-fg-muted"
				aria-expanded={trashOpen}
				onclick={() => {
					trashOpen = !trashOpen;
					if (trashOpen) void loadTrash();
				}}
			>
				{$t('flow.trash.title')}
			</button>
			{#if trashOpen}
				{#if trashItems.length === 0 && !trashLoading && !trashFailureKey}
					<p class="px-2 py-1 text-xs text-fg-muted">{$t('flow.trash.noItems')}</p>
				{/if}
				<ul class="space-y-1">
					{#each trashItems as item (item.id)}
						<li class="flex items-center gap-1 px-2 py-1 text-sm">
							<FlowObjectIcon appearance={item.appearance} objectId={item.id} />
							<span class="min-w-0 flex-1 truncate">{item.title || $t('flow.nav.untitled')}</span>
							<button
								type="button"
								class="text-xs underline"
								disabled={trashBusy}
								onclick={() => restoreArchived(item.id)}
							>
								{$t('flow.trash.restore')}
							</button>
							{#if confirmingDelete === item.id}
								<button
									type="button"
									class="text-xs text-danger underline"
									disabled={trashBusy}
									onclick={() => purgeArchived(item.id)}
								>
									{$t('flow.trash.confirmDelete')}
								</button>
							{:else}
								<button
									type="button"
									class="text-xs underline"
									disabled={trashBusy}
									onclick={() => (confirmingDelete = item.id)}
								>
									{$t('flow.trash.delete')}
								</button>
							{/if}
						</li>
					{/each}
				</ul>
				{#if trashLoading}<p class="px-2 text-xs text-fg-muted" role="status">
						{$t('common.loading')}
					</p>{/if}
				{#if trashNextCursor}
					<button
						type="button"
						class="min-h-11 px-2 text-sm underline"
						disabled={trashLoading || trashBusy}
						onclick={() => loadTrash(trashNextCursor)}>{$t('common.loadMore')}</button
					>
				{/if}
				{#if trashFailureKey}
					<p class="px-2 text-xs text-danger" role="alert">{$t(trashFailureKey)}</p>
				{/if}
			{/if}
		</div>
	</div>

	<div aria-live="polite" class="sr-only" role="status">{announcement}</div>
</nav>

<CreateCollectionDialog
	open={collectionDialogOpen}
	{scopeId}
	oncreated={onCollectionCreated}
	onclose={() => (collectionDialogOpen = false)}
/>

{#if contextMenuFor && contextMenuPos}
	<div
		class="fixed inset-0 z-40"
		role="presentation"
		onclick={closeContextMenu}
		oncontextmenu={(e) => {
			e.preventDefault();
			closeContextMenu();
		}}
	></div>
	<div
		class="fixed z-50 w-48 rounded-float border border-overlay-line bg-overlay py-1 text-sm shadow-overlay"
		style={`left: ${contextMenuPos.x}px; top: ${contextMenuPos.y}px`}
		role="menu"
		tabindex="-1"
		aria-label={$t('flow.nav.contextMenu.label')}
		onkeydown={(e) => {
			if (e.key === 'Escape') {
				e.preventDefault();
				closeContextMenu();
			}
		}}
	>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-fg hover:bg-surface"
			onclick={() => moveRelative(contextMenuFor!, 'before')}
		>
			{$t('flow.nav.contextMenu.moveBefore')}
		</button>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-fg hover:bg-surface"
			onclick={() => moveRelative(contextMenuFor!, 'after')}
		>
			{$t('flow.nav.contextMenu.moveAfter')}
		</button>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-fg hover:bg-surface"
			disabled={!integrationWorkspaceId &&
				!personalMoveAllowed(
					objects.find((object) => object.id === contextMenuFor)?.object_type,
					'page'
				)}
			onclick={() => openMoveInside(contextMenuFor!)}
		>
			{$t('flow.nav.contextMenu.moveInside')}
		</button>
		<button
			type="button"
			role="menuitem"
			class="block w-full px-3 py-1.5 text-left text-fg hover:bg-surface"
			disabled={!integrationWorkspaceId &&
				!personalMoveAllowed(
					objects.find((object) => object.id === contextMenuFor)?.object_type,
					'page'
				)}
			onclick={() => moveOutside(contextMenuFor!)}
		>
			{$t('flow.nav.contextMenu.moveOutside')}
		</button>
		<button
			type="button"
			class="block w-full px-3 py-2 text-left hover:bg-surface"
			role="menuitem"
			onclick={() => archiveObject(contextMenuFor!)}
		>
			{$t('flow.trash.archive')}
		</button>
	</div>
{/if}

{#if moveInsideFor}
	<Modal
		open
		title={$t('flow.nav.move.title')}
		maxWidthClass="max-w-sm"
		onclose={() => (moveInsideFor = null)}
	>
		{#if insideChoices.length === 0}
			<p class="text-sm text-fg-muted">{$t('flow.nav.move.empty')}</p>
		{:else}
			<ul class="max-h-64 space-y-1 overflow-y-auto">
				{#each insideChoices as choice (choice.id)}
					<li>
						<button
							type="button"
							class="w-full rounded-row px-2 py-1.5 text-left text-sm text-fg hover:bg-surface"
							onclick={() => {
								const sourceId = moveInsideFor;
								moveInsideFor = null;
								if (sourceId) void moveToParent(sourceId, choice.id);
							}}
						>
							{$t('flow.nav.move.choose', { values: { title: choice.title } })}
						</button>
					</li>
				{/each}
			</ul>
		{/if}
		<button type="button" class="mt-3 text-sm text-fg-muted" onclick={() => (moveInsideFor = null)}>
			{$t('common.cancel')}
		</button>
	</Modal>
{/if}
