export type ProgressState = 'queued' | 'running' | 'done' | 'failed';
export type Announcement = 'started' | 'milestone' | 'done' | 'failed';

/** Clamps a value to 0..max; a missing or non-numeric value counts as 0. */
export function clampValue(value: number, max: number): number {
	if (!(max > 0) || !Number.isFinite(max)) return 0;
	if (Number.isNaN(value)) return 0;
	return Math.min(max, Math.max(0, value));
}

/** The share done, rounded to a whole percentage in 0..100. */
export function percentOf(value: number, max: number): number {
	if (!(max > 0) || !Number.isFinite(max)) return 0;
	return Math.round((clampValue(value, max) / max) * 100);
}

/** Quarter steps: 0 below 25%, 1 below 50%, 2 below 75%, 3 below 100%, 4 when complete. */
export function stepOf(percent: number): number {
	return Math.min(4, Math.max(0, Math.floor(percent / 25)));
}

/**
 * What a polite live region should say for a change. Only the start, each quarter, completion
 * and failure are announced, never every change (specification T6, rule 5).
 */
export function announcementFor(
	previous: { state: ProgressState; step: number } | null,
	next: { state: ProgressState; step: number }
): Announcement | null {
	if (!previous) return null;
	if (previous.state !== next.state) {
		if (next.state === 'done') return 'done';
		if (next.state === 'failed') return 'failed';
		if (previous.state === 'queued' && next.state === 'running') return 'started';
		return null;
	}
	if (next.state === 'running' && next.step > previous.step) return 'milestone';
	return null;
}
