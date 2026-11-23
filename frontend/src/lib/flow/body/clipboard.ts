import { randomUuid } from '#lib/utils/uuid.js';

/** Only this live Page's own clipboard records may reuse its owned body references. */
const tickets = new Map<string, { owner: WeakRef<object>; blocks: string; expires: number }>();
export function issueBodyClipboard(owner: object, blocks: unknown[]): string {
	for (const [id, value] of tickets)
		if (value.expires < Date.now() || !value.owner.deref()) tickets.delete(id);
	while (tickets.size >= 20) tickets.delete(tickets.keys().next().value!);
	const id = randomUuid();
	tickets.set(id, {
		owner: new WeakRef(owner),
		blocks: JSON.stringify(blocks),
		expires: Date.now() + 300000
	});
	return id;
}
export function trustedBodyClipboard(owner: object, ticket: unknown, blocks: unknown[]): boolean {
	if (typeof ticket !== 'string') return false;
	const value = tickets.get(ticket);
	return (
		!!value &&
		value.owner.deref() === owner &&
		value.expires >= Date.now() &&
		value.blocks === JSON.stringify(blocks)
	);
}
export function clearBodyClipboard(owner: object): void {
	for (const [id, value] of cutTickets) if (value.owner.deref() === owner) cutTickets.delete(id);
	for (const [id, value] of tickets) if (value.owner.deref() === owner) tickets.delete(id);
}

/** Opaque whole-block cut intents never accept serialized CIDs as authority. */
const cutTickets = new Map<string, { owner: WeakRef<object>; value: object; expires: number }>();
export function issueBlockCutTicket(owner: object, value: object): string {
	for (const [id, entry] of cutTickets)
		if (entry.expires < Date.now() || !entry.owner.deref()) cutTickets.delete(id);
	while (cutTickets.size >= 20) cutTickets.delete(cutTickets.keys().next().value!);
	const ticket = randomUuid();
	cutTickets.set(ticket, { owner: new WeakRef(owner), value, expires: Date.now() + 300000 });
	return ticket;
}
export function blockCutTicket<T extends object>(owner: object, ticket: unknown): T | null {
	const entry = typeof ticket === 'string' ? cutTickets.get(ticket) : undefined;
	return entry && entry.owner.deref() === owner && entry.expires >= Date.now()
		? (entry.value as T)
		: null;
}
export function consumeBlockCutTicket(ticket: string): void {
	cutTickets.delete(ticket);
}
