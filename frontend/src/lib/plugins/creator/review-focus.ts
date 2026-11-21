/**
 * Moves keyboard and screen-reader focus to the review heading once a build has produced a
 * review, without scrolling. The heading is mounted together with the preview frame, whose height
 * changes while its content loads. A focus-driven scroll lands in the middle of that, and the
 * review controls below the frame jump under the pointer: a click made right after the preview
 * shows reaches the frame instead of the control.
 */
export function focusReviewStart(heading: Pick<HTMLElement, 'focus'> | null | undefined): void {
	heading?.focus({ preventScroll: true });
}
