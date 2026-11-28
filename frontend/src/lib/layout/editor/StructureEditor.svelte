<svelte:options runes />

<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import Redo2 from '@lucide/svelte/icons/redo-2';
	import Undo2 from '@lucide/svelte/icons/undo-2';
	import Button from '#lib/ui/button/Button.svelte';
	import type { LayoutSpec, LayoutValidationError, SlotDescriptor } from '../types';
	import StructureNode from './StructureNode.svelte';
	import {
		EMPTY_HISTORY,
		canRedo,
		canUndo,
		defaultNode,
		dropIndex,
		errorsByNode,
		insertOp,
		moveOp,
		nodeAt,
		paletteFor,
		perform,
		redo,
		removeOp,
		setColumnsOp,
		undo,
		type LayoutOp,
		type OpHistory,
		type StructureCtrl
	} from './ops';

	/**
	 * The structural mode of the layout editor: the node tree as boxes, grid children dragged (or
	 * moved with the arrow keys on the grip) to reorder, the grid's column count set on a
	 * four-step ruler, nodes added from the palette and removed. Every edit is an op of the
	 * invertible log in `ops.ts`; undo and redo walk that log. The spec is owned by the parent: each
	 * edit hands it the new spec, and a spec that did not come from this editor starts a new log.
	 */
	let {
		spec,
		errors,
		slot,
		onchange
	}: {
		spec: LayoutSpec;
		errors: readonly LayoutValidationError[];
		slot: Pick<SlotDescriptor, 'stack_children'>;
		onchange: (spec: LayoutSpec) => void;
	} = $props();

	let history = $state.raw<OpHistory>(EMPTY_HISTORY);
	let working = $state.raw<LayoutSpec>(untrack(() => spec));
	let lastEmitted = untrack(() => JSON.stringify(spec));
	let drag = $state.raw<{ parent: string; from: number; to: number } | null>(null);
	let status = $state('');
	let rootEl: HTMLElement | undefined = $state();

	$effect(() => {
		const incoming = JSON.stringify(spec);
		if (incoming === lastEmitted) return;
		lastEmitted = incoming;
		working = spec;
		history = EMPTY_HISTORY;
	});

	const byNode = $derived(errorsByNode(errors));
	const undoable = $derived(canUndo(history));
	const redoable = $derived(canRedo(history));

	function publish(next: LayoutSpec) {
		working = next;
		lastEmitted = JSON.stringify(next);
		onchange(next);
	}

	function describe(op: LayoutOp): string {
		const target = op.path.replace(/^\/root/, '') || '/';
		return $t(`layout.editor.structure.op.${op.op}`, { values: { path: target } });
	}

	function commit(op: LayoutOp | null): boolean {
		if (!op) return false;
		const done = perform(working, history, op);
		if (!done) {
			status = $t('layout.editor.structure.rejected');
			return false;
		}
		history = done.history;
		publish(done.spec);
		status = describe(op);
		return true;
	}

	function stepUndo() {
		const step = undo(working, history);
		if (!step) return;
		history = step.history;
		publish(step.spec);
		status = $t('layout.editor.structure.undone', { values: { what: describe(step.op) } });
	}

	function stepRedo() {
		const step = redo(working, history);
		if (!step) return;
		history = step.history;
		publish(step.spec);
		status = $t('layout.editor.structure.redone', { values: { what: describe(step.op) } });
	}

	function onkeydown(event: KeyboardEvent) {
		if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
		const target = event.target;
		if (
			target instanceof HTMLElement &&
			(target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
		) {
			return;
		}
		const key = event.key.toLowerCase();
		if (key === 'z') {
			event.preventDefault();
			if (event.shiftKey) stepRedo();
			else stepUndo();
		} else if (key === 'y' && !event.shiftKey) {
			event.preventDefault();
			stepRedo();
		}
	}

	async function focusGrip(path: string) {
		await tick();
		const grip = [...(rootEl?.querySelectorAll<HTMLElement>('[data-grip]') ?? [])].find(
			(item) => item.dataset.grip === path
		);
		grip?.focus();
	}

	function endDrag() {
		window.removeEventListener('pointermove', dragMove);
		window.removeEventListener('pointerup', dragUp);
		window.removeEventListener('pointercancel', dragCancel);
		drag = null;
		activeList = null;
	}

	let activeList: HTMLElement | null = null;

	function dragMove(event: PointerEvent) {
		if (!drag || !activeList) return;
		const boxes = [...activeList.children].map((child) => child.getBoundingClientRect());
		const to = dropIndex(boxes, event.clientX, event.clientY);
		if (to !== drag.to) drag = { ...drag, to };
	}

	function dragUp() {
		const done = drag;
		endDrag();
		if (done && done.from !== done.to) {
			if (commit(moveOp(done.parent, working, done.from, done.to))) {
				void focusGrip(`${done.parent}/children/${done.to}`);
			}
		}
	}

	function dragCancel() {
		endDrag();
	}

	const ctrl: StructureCtrl = {
		errorsAt: (path) => byNode.get(path) ?? [],
		palette: (path) => paletteFor(working, path, slot),
		dragging: () => drag,
		beginDrag(parent, from, list, event) {
			if (drag) return;
			event.preventDefault();
			activeList = list;
			drag = { parent, from, to: from };
			window.addEventListener('pointermove', dragMove);
			window.addEventListener('pointerup', dragUp);
			window.addEventListener('pointercancel', dragCancel);
		},
		move(parent, from, to) {
			if (commit(moveOp(parent, working, from, to))) void focusGrip(`${parent}/children/${to}`);
		},
		remove(path) {
			commit(removeOp(working, path));
		},
		add(parent, type) {
			const count = nodeAt(working, parent)?.children?.length ?? 0;
			if (commit(insertOp(parent, working, count, defaultNode(type)))) {
				void focusGrip(`${parent}/children/${count}`);
			}
		},
		setColumns(path, columns) {
			commit(setColumnsOp(working, path, columns));
		}
	};
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
	bind:this={rootEl}
	class="min-w-0"
	data-structure-editor
	aria-label={$t('layout.editor.structure.title')}
	{onkeydown}
>
	<div class="mb-2 flex flex-wrap items-center gap-2">
		<Button
			size="sm"
			disabled={!undoable}
			aria-label={$t('layout.editor.structure.undo')}
			title={$t('layout.editor.structure.undoHint')}
			data-structure-undo
			onclick={stepUndo}
		>
			<Undo2 size={14} aria-hidden="true" />
		</Button>
		<Button
			size="sm"
			disabled={!redoable}
			aria-label={$t('layout.editor.structure.redo')}
			title={$t('layout.editor.structure.redoHint')}
			data-structure-redo
			onclick={stepRedo}
		>
			<Redo2 size={14} aria-hidden="true" />
		</Button>
		<span class="text-xs text-fg-muted" data-structure-history>
			{$t('layout.editor.structure.history', {
				values: { done: history.cursor, total: history.ops.length }
			})}
		</span>
	</div>
	<p id="structure-help" class="mb-2 text-xs text-fg-muted">
		{$t('layout.editor.structure.help')}
	</p>
	<StructureNode node={working.root} path="/root" {ctrl} />
	<p class="sr-only" role="status" aria-live="polite" data-structure-status>{status}</p>
</section>
