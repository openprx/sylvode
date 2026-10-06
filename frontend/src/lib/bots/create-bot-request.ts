/**
 * The request the members page sends to create a bot token, built from the form's state.
 *
 * Kept out of the page so the rule it encodes can be tested without a browser: the token is bound
 * to exactly the surface the user chose (the API binds every credential to one surface and
 * refuses it on any other), the form refuses to submit without a choice instead of letting the
 * API fall back to `rest`, and a token needs a name. `tests/create-bot-request.test.ts` covers it.
 */
import type { CreateBotData } from '$lib/api/bots';
import type { BotTransportSurface } from './transport-surfaces';

export interface CreateBotForm {
	name: string;
	permissions: readonly string[];
	/** `''` until the user picks a surface. */
	surface: BotTransportSurface | '';
	expiresAt?: string;
}

export type CreateBotRequestResult =
	| { ok: true; request: CreateBotData }
	| { ok: false; errorKey: 'members.tokenNameRequired' | 'members.transportSurfaceRequired' };

export function createBotRequest(form: CreateBotForm): CreateBotRequestResult {
	const name = form.name.trim();
	if (!name) return { ok: false, errorKey: 'members.tokenNameRequired' };
	if (form.surface === '') return { ok: false, errorKey: 'members.transportSurfaceRequired' };
	return {
		ok: true,
		request: {
			name,
			permissions: [...form.permissions],
			transport_surface: form.surface,
			expires_at: form.expiresAt
		}
	};
}
