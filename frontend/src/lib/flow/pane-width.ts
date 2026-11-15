export function paneBounds(side: 'left' | 'right', containerWidth: number) {
	const nominal =
		side === 'left' ? { min: 200, max: 400, initial: 256 } : { min: 280, max: 520, initial: 352 };
	const max = Math.max(
		120,
		Math.min(nominal.max, Math.min(Math.floor(containerWidth * 0.38), containerWidth - 360))
	);
	return { ...nominal, min: Math.min(nominal.min, max), max };
}
export function clampPaneWidth(value: number, min: number, max: number) {
	return Math.round(Math.max(min, Math.min(max, Number.isFinite(value) ? value : min)));
}
