/**
 * How the status of a connection is shown: the chip of every state, relative times, the last
 * delivery line, and when the section keeps asking while it waits for the runner.
 */
import type { ComponentState, DeliveryState, RunnerState, Status } from './wire';

export type Tone = 'success' | 'warning' | 'danger' | 'neutral';
export interface Chip {
	tone: Tone;
	key: string;
}

const KEY = 'execution.connection.status.';

export function runnerChip(state: RunnerState): Chip {
	switch (state) {
		case 'online':
			return { tone: 'success', key: `${KEY}runner.online` };
		case 'waiting':
			return { tone: 'warning', key: `${KEY}runner.waiting` };
		case 'not_reporting':
			return { tone: 'warning', key: `${KEY}runner.notReporting` };
		case 'unreachable':
			return { tone: 'danger', key: `${KEY}runner.unreachable` };
	}
}

/** The CLI and tmux rows share their states. */
export function componentChip(state: ComponentState): Chip {
	switch (state) {
		case 'ready':
			return { tone: 'success', key: `${KEY}component.ready` };
		case 'warning':
			return { tone: 'warning', key: `${KEY}component.warning` };
		case 'missing':
			return { tone: 'danger', key: `${KEY}component.missing` };
		case 'disabled':
			return { tone: 'danger', key: `${KEY}component.disabled` };
		case 'unknown':
			return { tone: 'neutral', key: `${KEY}component.unknown` };
	}
}

export function deliveryChip(state: DeliveryState): Chip {
	switch (state) {
		case 'delivered':
			return { tone: 'success', key: `${KEY}delivery.delivered` };
		case 'refused':
			return { tone: 'warning', key: `${KEY}delivery.refused` };
		case 'failed':
			return { tone: 'danger', key: `${KEY}delivery.failed` };
		case 'none':
			return { tone: 'neutral', key: `${KEY}delivery.none` };
	}
}

const UNITS: Array<[Intl.RelativeTimeFormatUnit, number]> = [
	['day', 86_400],
	['hour', 3_600],
	['minute', 60],
	['second', 1]
];

/** "5 minutes ago", in the app's language; `''` for a time that does not parse. */
export function relativeTime(iso: string | null, now: Date, locale: string): string {
	if (iso === null) return '';
	const then = Date.parse(iso);
	if (Number.isNaN(then)) return '';
	const seconds = Math.round((then - now.getTime()) / 1000);
	const format = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
	for (const [unit, size] of UNITS) {
		if (Math.abs(seconds) >= size || unit === 'second')
			return format.format(Math.trunc(seconds / size), unit);
	}
	return '';
}

/** "{time} · {project key} · {reason code}", the parts that are known. */
export function deliveryValue(
	delivery: Status['last_delivery'],
	now: Date,
	locale: string
): string {
	if (delivery.state === 'none') return '';
	return [relativeTime(delivery.at, now, locale), delivery.project_key, delivery.reason_code]
		.filter((part): part is string => typeof part === 'string' && part !== '')
		.join(' · ');
}

/** The status is asked again every this many seconds while the runner has not shown up... */
export const WAITING_POLL_SECONDS = 15;
/** ...for at most this long. */
export const WAITING_POLL_LIMIT_SECONDS = 600;

export function shouldPollWaiting(status: Status | null, waitedSeconds: number): boolean {
	return status?.runner.state === 'waiting' && waitedSeconds < WAITING_POLL_LIMIT_SECONDS;
}
