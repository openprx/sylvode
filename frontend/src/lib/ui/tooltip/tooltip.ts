import { mount, unmount } from 'svelte';
import { uid } from '../foundations/ids';
import { pushLayer } from '../foundations/layers';
import Tooltip from './Tooltip.svelte';
import { createTooltipController } from './tooltip-controller';

export interface TooltipParams {
	text: string;
	shortcut?: string;
	/** True when the tooltip adds information beyond the anchor's accessible name. */
	describes?: boolean;
}

/**
 * Svelte action: `use:tooltip={{ text, shortcut?, describes? }}`. Shows after a 500 ms hover or
 * at once on keyboard focus; Escape hides it without closing the layer around it. Touch gets
 * none. No Tab stop.
 */
export function tooltip(node: HTMLElement, params: TooltipParams) {
	const id = uid('tooltip');
	let current = params;
	let instance: Record<string, unknown> | null = null;
	let removeLayer: (() => void) | null = null;
	let pointerFocus = false;

	function show() {
		if (instance || !current.text) return;
		instance = mount(Tooltip, {
			target: document.body,
			props: {
				id,
				anchor: node,
				text: current.text,
				shortcut: current.shortcut,
				describes: current.describes,
				onpointerenter: () => controller.tooltipEnter(),
				onpointerleave: () => controller.tooltipLeave()
			}
		});
		if (current.describes) node.setAttribute('aria-describedby', id);
		removeLayer = pushLayer({ onEscape: () => controller.hide() });
	}
	function hide() {
		if (!instance) return;
		void unmount(instance);
		instance = null;
		removeLayer?.();
		removeLayer = null;
		if (node.getAttribute('aria-describedby') === id) node.removeAttribute('aria-describedby');
	}

	const controller = createTooltipController({
		onchange: (visible) => (visible ? show() : hide())
	});

	const onEnter = (event: PointerEvent) => {
		if (event.pointerType === 'touch') return;
		controller.pointerEnter();
	};
	const onLeave = () => controller.pointerLeave();
	const onPointerDown = () => {
		pointerFocus = true;
		controller.hide();
	};
	const onFocus = () => {
		const keyboard = !pointerFocus && node.matches(':focus-visible');
		pointerFocus = false;
		if (keyboard) controller.focus();
	};
	const onBlur = () => controller.hide();
	const onScroll = () => controller.hide();

	node.addEventListener('pointerenter', onEnter);
	node.addEventListener('pointerleave', onLeave);
	node.addEventListener('pointerdown', onPointerDown);
	node.addEventListener('focus', onFocus);
	node.addEventListener('blur', onBlur);
	window.addEventListener('scroll', onScroll, true);

	return {
		update(next: TooltipParams) {
			current = next;
			if (instance) {
				hide();
				show();
			}
		},
		destroy() {
			controller.hide();
			hide();
			node.removeEventListener('pointerenter', onEnter);
			node.removeEventListener('pointerleave', onLeave);
			node.removeEventListener('pointerdown', onPointerDown);
			node.removeEventListener('focus', onFocus);
			node.removeEventListener('blur', onBlur);
			window.removeEventListener('scroll', onScroll, true);
		}
	};
}
