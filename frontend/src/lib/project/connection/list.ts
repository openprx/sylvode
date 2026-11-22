/**
 * The list of connections a project shows, as plain functions over a plain array: a write
 * replaces its own card, a removal drops it, and the order stays that of the API (by bot name).
 */
import type { ConnectionItem } from './wire';

function byBotName(a: ConnectionItem, b: ConnectionItem): number {
	return a.connection.bot.name.localeCompare(b.connection.bot.name);
}

/** The list with this connection's card replaced, or added when the bot had none. */
export function putItem(items: readonly ConnectionItem[], item: ConnectionItem): ConnectionItem[] {
	const id = item.connection.bot.id;
	const known = items.some((entry) => entry.connection.bot.id === id);
	const next = known
		? items.map((entry) => (entry.connection.bot.id === id ? item : entry))
		: [...items, item];
	return known ? next : next.sort(byBotName);
}

export function dropItem(items: readonly ConnectionItem[], botId: string): ConnectionItem[] {
	return items.filter((entry) => entry.connection.bot.id !== botId);
}

/** Whether any card still waits for its runner's first report (the section keeps asking). */
export function anyWaiting(items: readonly ConnectionItem[]): boolean {
	return items.some((entry) => entry.status?.runner.state === 'waiting');
}
