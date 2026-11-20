/**
 * Frame state of the Creator's v3 preview. The preview is fixed to one draft revision (the
 * caller remounts it for a new one), so the only thing that changes is which frame is shown:
 * `attempt` keys the host, so every open mounts its own, and `closed` is the notice shown
 * instead of a frame (`''` is closed without a reason).
 */
export interface PreviewFrame {
	attempt: number;
	closed: string | null;
}

export const initialPreviewFrame = (): PreviewFrame => ({ attempt: 0, closed: null });

/** The host closed the frame with a reason. */
export function closePreview(state: PreviewFrame, reason: string) {
	state.closed = reason;
}

/** The user asked to try again. */
export function retryPreview(state: PreviewFrame) {
	state.closed = null;
	state.attempt++;
}

/**
 * The host of `attempt` retired its frame silently while the page was being left and is live
 * again. The preview mounts a fresh frame for the same draft, unless that host is no longer the
 * one shown. Returns whether it reopened.
 *
 * Nothing is re-earned: whether the preview ran is attested per draft revision by the caller
 * (a new revision remounts the preview and resets it), not per frame, and the new frame shows the
 * identical draft.
 */
export function retirePreview(state: PreviewFrame, attempt: number): boolean {
	if (state.closed !== null || state.attempt !== attempt) return false;
	state.attempt++;
	return true;
}
