/** The agent types the runner knows; an administrator picks one when creating a bot user. */
export const AGENT_TYPES = ['runner', 'openclaw', 'openprx', 'webhook', 'custom'] as const;

export type AgentType = (typeof AGENT_TYPES)[number];

export function isAgentType(value: unknown): value is AgentType {
	return typeof value === 'string' && (AGENT_TYPES as readonly string[]).includes(value);
}

/** Message key of an agent type's name, or null for bots without one (token and connection bots). */
export function agentTypeKey(value: string | null | undefined): string | null {
	return isAgentType(value) ? `agentTypes.${value}` : null;
}
