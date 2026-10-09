/**
 * The transport surfaces a workspace bot token can be bound to.
 *
 * The API binds every bot credential to exactly one surface when it is created
 * (`workspace_bots.transport_surface`) and refuses it on any other: a token created for `rest`
 * gets 401 through the MCP server and through the CLI. The list mirrors the server's own
 * allow-list `EventSurface::BOT_CREDENTIAL_SURFACES` (`apps/api/src/flow/event_origin.rs`), in the same order;
 * `tests/bot-transport-surfaces.test.ts` fails when the two drift apart.
 *
 * Every surface has a label and a one-line description under `members.surface.<surface>` in
 * each locale file.
 */
export const BOT_TRANSPORT_SURFACES = [
	'rest',
	'mcp_http',
	'mcp_sse',
	'mcp_stdio',
	'cli',
	'cli_tools_call'
] as const;

export type BotTransportSurface = (typeof BOT_TRANSPORT_SURFACES)[number];

export function isBotTransportSurface(value: string): value is BotTransportSurface {
	return (BOT_TRANSPORT_SURFACES as readonly string[]).includes(value);
}

/** The i18n key of a surface's short label. */
export function surfaceLabelKey(surface: BotTransportSurface): string {
	return `members.surface.${surface}.label`;
}

/** The i18n key of a surface's one-line explanation of where the token works. */
export function surfaceDescriptionKey(surface: BotTransportSurface): string {
	return `members.surface.${surface}.description`;
}
