/**
 * The stack of open floating layers (tooltip, popover, menu, dialog). Escape closes only the
 * topmost one, and stops there, so a tooltip over a popover over a dialog unwinds one at a time.
 */

export interface Layer {
	/** Called when Escape reaches this layer; the layer closes itself. */
	onEscape: () => void;
}

const stack: Layer[] = [];
let listening = false;

function onKeydown(event: KeyboardEvent) {
	if (event.key !== 'Escape' || stack.length === 0) return;
	event.preventDefault();
	event.stopPropagation();
	stack[stack.length - 1].onEscape();
}

/** Opens a layer; returns the function that removes it. Safe to call the remover twice. */
export function pushLayer(layer: Layer): () => void {
	stack.push(layer);
	if (!listening && typeof document !== 'undefined') {
		// Capture phase: runs before any handler inside the page, so the topmost layer wins.
		document.addEventListener('keydown', onKeydown, true);
		listening = true;
	}
	return () => {
		const index = stack.indexOf(layer);
		if (index >= 0) stack.splice(index, 1);
		if (stack.length === 0 && listening) {
			document.removeEventListener('keydown', onKeydown, true);
			listening = false;
		}
	};
}

export function isTopLayer(layer: Layer): boolean {
	return stack[stack.length - 1] === layer;
}

export function openLayerCount(): number {
	return stack.length;
}
