/**
 * Durations in seconds as a number and a unit. The unit is the largest one that divides the
 * stored value exactly, so saving never changes a value nobody edited (3600 is 1 hour, 90 is
 * 90 seconds).
 */
export const DURATION_UNITS = ['seconds', 'minutes', 'hours'] as const;
export type DurationUnit = (typeof DURATION_UNITS)[number];

const FACTOR: Record<DurationUnit, number> = { seconds: 1, minutes: 60, hours: 3600 };

export interface Duration {
	amount: number;
	unit: DurationUnit;
}

export function toDuration(seconds: number): Duration {
	if (seconds > 0 && seconds % 3600 === 0) return { amount: seconds / 3600, unit: 'hours' };
	if (seconds > 0 && seconds % 60 === 0) return { amount: seconds / 60, unit: 'minutes' };
	return { amount: seconds, unit: 'seconds' };
}

/** Seconds of an amount and a unit; `null` unless the amount is a whole, non-negative number. */
export function toSeconds(amount: number, unit: DurationUnit): number | null {
	return Number.isInteger(amount) && amount >= 0 ? amount * FACTOR[unit] : null;
}

/** Re-expresses the same length in another unit when it divides exactly, else keeps the number. */
export function changeUnit(current: Duration, unit: DurationUnit): Duration {
	const seconds = toSeconds(current.amount, current.unit);
	if (seconds === null) return { amount: current.amount, unit };
	const factor = FACTOR[unit];
	return seconds % factor === 0
		? { amount: seconds / factor, unit }
		: { amount: current.amount, unit };
}
