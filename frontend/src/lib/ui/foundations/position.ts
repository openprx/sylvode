export type Placement = 'bottom-start' | 'bottom-end' | 'top-start' | 'top-end' | 'bottom' | 'top';

export interface Box {
	left: number;
	top: number;
	width: number;
	height: number;
}

export interface PositionInput {
	anchor: Box;
	floating: { width: number; height: number };
	viewport: { width: number; height: number };
	placement?: Placement;
	/** Gap between anchor and floating element. */
	offset?: number;
	/** Minimum distance kept from the viewport edge. */
	margin?: number;
	/** Right-to-left layout mirrors `-start` and `-end`. */
	rtl?: boolean;
}

export interface Position {
	x: number;
	y: number;
	placement: Placement;
}

/**
 * Pure placement: below or above the anchor, aligned to its start, end or centre, flipped to the
 * other side when it would not fit and the other side has more room, then clamped to the
 * viewport. Returns viewport coordinates for `position: fixed`.
 */
export function computePosition({
	anchor,
	floating,
	viewport,
	placement = 'bottom-start',
	offset = 6,
	margin = 8,
	rtl = false
}: PositionInput): Position {
	const [side, align = 'center'] = placement.split('-') as ['top' | 'bottom', ('start' | 'end')?];
	const roomBelow = viewport.height - (anchor.top + anchor.height) - margin;
	const roomAbove = anchor.top - margin;
	const need = floating.height + offset;
	let finalSide = side;
	if (side === 'bottom' && roomBelow < need && roomAbove > roomBelow) finalSide = 'top';
	else if (side === 'top' && roomAbove < need && roomBelow > roomAbove) finalSide = 'bottom';

	const startEdge = (align === 'start') !== rtl;
	let x: number;
	if (align === 'center') x = anchor.left + anchor.width / 2 - floating.width / 2;
	else x = startEdge ? anchor.left : anchor.left + anchor.width - floating.width;
	x = Math.max(margin, Math.min(x, viewport.width - floating.width - margin));

	let y =
		finalSide === 'bottom'
			? anchor.top + anchor.height + offset
			: anchor.top - floating.height - offset;
	y = Math.max(margin, Math.min(y, viewport.height - floating.height - margin));

	const resolved = (align === 'center' ? finalSide : `${finalSide}-${align}`) as Placement;
	return { x: Math.round(x), y: Math.round(y), placement: resolved };
}

/**
 * Measures `anchor` and `floating`, applies `computePosition` as `left`/`top` on `floating`, and
 * repeats on scroll and resize. Returns the stop function.
 */
export function autoPosition(
	anchor: HTMLElement,
	floating: HTMLElement,
	options: Pick<PositionInput, 'placement' | 'offset' | 'margin'> = {},
	onplaced?: (position: Position) => void
): () => void {
	function update() {
		const a = anchor.getBoundingClientRect();
		const position = computePosition({
			...options,
			anchor: { left: a.left, top: a.top, width: a.width, height: a.height },
			floating: { width: floating.offsetWidth, height: floating.offsetHeight },
			viewport: { width: window.innerWidth, height: window.innerHeight },
			rtl: getComputedStyle(anchor).direction === 'rtl'
		});
		floating.style.left = `${position.x}px`;
		floating.style.top = `${position.y}px`;
		onplaced?.(position);
	}
	update();
	window.addEventListener('resize', update);
	window.addEventListener('scroll', update, true);
	return () => {
		window.removeEventListener('resize', update);
		window.removeEventListener('scroll', update, true);
	};
}
