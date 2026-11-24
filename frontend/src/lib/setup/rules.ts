/** The shortest password the server accepts. */
export const MIN_PASSWORD_LENGTH = 8;

export interface SetupForm {
	name: string;
	email: string;
	password: string;
	confirm: string;
}

export type SetupErrorKey =
	| 'setup.errors.nameRequired'
	| 'setup.errors.emailInvalid'
	| 'setup.errors.passwordShort'
	| 'setup.errors.passwordMismatch';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** The first problem of the first-run form as a locale key, or null when it can be submitted. */
export function validateSetup(form: SetupForm): SetupErrorKey | null {
	if (!form.name.trim()) return 'setup.errors.nameRequired';
	if (!EMAIL.test(form.email.trim())) return 'setup.errors.emailInvalid';
	if (form.password.length < MIN_PASSWORD_LENGTH) return 'setup.errors.passwordShort';
	if (form.password !== form.confirm) return 'setup.errors.passwordMismatch';
	return null;
}

/** The body of the registration request; the trimmed name and e-mail, the password as typed. */
export function setupBody(form: SetupForm): { name: string; email: string; password: string } {
	return { name: form.name.trim(), email: form.email.trim(), password: form.password };
}
