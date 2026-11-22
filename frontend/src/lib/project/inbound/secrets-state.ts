/**
 * The one-time credentials of an inbound source as a state machine.
 *
 * A source create or rotate answer holds the token and the signing secret, and the API shows them
 * exactly once. They therefore live in one place only: the `SecretsState` value a component keeps
 * in its own state while the credentials are on screen. They are never put in a store, in
 * storage, in the URL, in a log or in an error report, and leaving the "shown" phase drops the
 * only reference to the values.
 *
 * Plain functions over a plain value (no class, no store, no closure over the values), so the
 * rules are unit-tested directly and a snapshot of any other state can never contain a value.
 * Same rules as `connection/block-state.ts`, kept separate so the two features change apart.
 */

export interface SecretValues {
	endpoint: string | null;
	token: string;
	signingSecret: string;
}

export type SecretsState =
	| { phase: 'none' }
	| { phase: 'shown'; values: SecretValues; stored: boolean };

export const NO_SECRETS: SecretsState = { phase: 'none' };

/** The credentials of a create or rotate answer, not yet confirmed as stored. */
export function showSecrets(values: SecretValues): SecretsState {
	return values.token === '' && values.signingSecret === ''
		? NO_SECRETS
		: { phase: 'shown', values, stored: false };
}

/** The "I have stored these credentials" checkbox. */
export function setStored(state: SecretsState, stored: boolean): SecretsState {
	return state.phase === 'shown' ? { phase: 'shown', values: state.values, stored } : state;
}

/** "Done" is available only once the person confirmed the credentials are stored. */
export function canFinish(state: SecretsState): boolean {
	return state.phase === 'shown' && state.stored;
}

/** "Done": drops the values, but only after the confirmation. */
export function finish(state: SecretsState): SecretsState {
	return canFinish(state) ? NO_SECRETS : state;
}

/** Leaving after the person agreed to lose the credentials: drops the values. */
export function discard(): SecretsState {
	return NO_SECRETS;
}

/** Leaving the page (in-app navigation or the browser) asks while the credentials are unconfirmed. */
export function needsLeaveConfirmation(state: SecretsState): boolean {
	return state.phase === 'shown' && !state.stored;
}

/** The values on screen, `null` when there are none. */
export function visibleValues(state: SecretsState): SecretValues | null {
	return state.phase === 'shown' ? state.values : null;
}

/** The text "Copy all as JSON" puts on the clipboard (the shape a prx-waf Sylvode channel takes). */
export function credentialsJson(values: SecretValues, fallbackUrl: string): string {
	return JSON.stringify(
		{
			url: values.endpoint ?? fallbackUrl,
			token: values.token,
			signing_secret: values.signingSecret
		},
		null,
		2
	);
}

/** The address shown when the server could not tell its public address (rules 2.3). */
export const ENDPOINT_PATH = '/api/v1/inbound/events';

export function guessedEndpoint(origin: string): string {
	return `${origin}${ENDPOINT_PATH}`;
}
