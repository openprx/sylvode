import { getContext } from 'svelte';
export const TASK_EDIT_SURFACE = 'sv-task-edit-surface';
export interface TaskEditSurface {
	mount(node: HTMLElement): void;
	unmount(node: HTMLElement): void;
}
/** Move only an edit sheet into the active panel's overlay, never another app or route. */
export function createTaskEditSurface(
	host: () => HTMLElement | null,
	background: () => HTMLElement | null,
	changed: (count: number) => void
): TaskEditSurface {
	const nodes = new Set<HTMLElement>();
	const previous = new Map<HTMLElement, HTMLElement | null>();
	return {
		mount(node) {
			const target = host();
			if (!target) return;
			const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
			previous.set(node, active);
			const focusFromPage = !!active && !!background()?.contains(active);
			target.appendChild(node);
			nodes.add(node);
			changed(nodes.size);
			if (focusFromPage)
				requestAnimationFrame(() => {
					if (node.isConnected)
						(
							node.querySelector<HTMLElement>('input:not([type="hidden"]),textarea,select') ??
							node.querySelector<HTMLElement>('button')
						)?.focus({ preventScroll: true });
				});
		},
		unmount(node) {
			const hadFocus = node.contains(document.activeElement);
			const prior = previous.get(node);
			nodes.delete(node);
			previous.delete(node);
			changed(nodes.size);
			if (hadFocus && prior)
				requestAnimationFrame(() => {
					if (prior.isConnected && !prior.closest('[inert]')) prior.focus({ preventScroll: true });
				});
			node.remove();
		}
	};
}
export function taskEditorAction() {
	const surface = getContext<TaskEditSurface | undefined>(TASK_EDIT_SURFACE);
	return (node: HTMLElement, enabled = true) => {
		if (!enabled || !surface) return;
		surface.mount(node);
		return { destroy: () => surface.unmount(node) };
	};
}
