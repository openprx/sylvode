/** Only the canvas/landing routes own an inner Flow scrolling viewport.
 * Conversion and package forms keep the standard application page scroller. */
export function isFlowCanvasRoute(routeId: string | null): boolean {
	return /^\/\(app\)\/(?:workspace\/\[workspaceId\]\/)?flow(?:\/\[objectId\])?$/.test(
		routeId ?? ''
	);
}
