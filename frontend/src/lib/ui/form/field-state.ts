import { createSubscriber } from 'svelte/reactivity';

/** Reactive bookkeeping shared by the two state classes: reads inside an effect re-run on change. */
class Observable {
	#listeners = new Set<() => void>();
	#track = createSubscriber((update) => {
		this.#listeners.add(update);
		return () => this.#listeners.delete(update);
	});
	protected track(): void {
		this.#track();
	}
	protected changed(): void {
		for (const listener of this.#listeners) listener();
	}
}

export interface FieldOptions {
	/** A required field is validated on blur even if it was never edited. */
	required?: boolean;
}

/**
 * One field's value and its error timing (batch A, T5):
 * - validated on blur (when the value changed, or the field is required) and on a submit attempt;
 * - never while typing the first time; once an error has been shown it re-validates on every
 *   input and clears as soon as the value is valid;
 * - a server error is shown as given and the typed value is kept.
 */
export class FieldState<T> extends Observable {
	#value: T;
	#touched = false;
	#dirty = false;
	#armed = false;
	#shown: string | null = null;
	readonly #validate: (value: T) => string | null;
	readonly #options: FieldOptions;

	constructor(initial: T, validate: (value: T) => string | null, options: FieldOptions = {}) {
		super();
		this.#value = initial;
		this.#validate = validate;
		this.#options = options;
	}

	get value(): T {
		this.track();
		return this.#value;
	}
	set value(next: T) {
		this.input(next);
	}
	get touched(): boolean {
		this.track();
		return this.#touched;
	}
	/** The message the user sees, or `null`. */
	get shownError(): string | null {
		this.track();
		return this.#shown;
	}
	/** The current validation result, whether or not it is shown yet. */
	get error(): string | null {
		this.track();
		return this.#validate(this.#value);
	}
	get valid(): boolean {
		return this.error === null;
	}

	input(next: T): void {
		this.#value = next;
		this.#dirty = true;
		if (this.#armed) this.#shown = this.#validate(next);
		this.changed();
	}

	blur(): void {
		this.#touched = true;
		if (this.#dirty || this.#options.required) this.#show(this.#validate(this.#value));
		this.changed();
	}

	/** Validates and shows the result; returns whether the value is valid. */
	submitAttempt(): boolean {
		this.#touched = true;
		const message = this.#validate(this.#value);
		this.#show(message);
		this.changed();
		return message === null;
	}

	setServerError(message: string): void {
		this.#show(message);
		this.changed();
	}

	#show(message: string | null): void {
		this.#shown = message;
		if (message !== null) this.#armed = true;
	}
}

export type Marking = 'required' | 'optional';

export interface MissingField {
	id: string;
	label: string;
}

/** The fields of one form, in display order, and the form-level questions about them. */
export class FormState extends Observable {
	/** Which kind of field carries a word: the minority one (batch A, T5). */
	marking: Marking;
	fields: Record<string, FieldState<unknown>> = {};
	#labels: Record<string, string> = {};

	constructor(marking: Marking = 'required') {
		super();
		this.marking = marking;
	}

	/** Adds a field. Order of registration is the order of the reason line and of focus. */
	register<T>(id: string, label: string, field: FieldState<T>): FieldState<T> {
		this.fields[id] = field as FieldState<unknown>;
		this.#labels[id] = label;
		this.changed();
		return field;
	}

	/** Fields whose current value does not validate, in registration order. */
	missing(): MissingField[] {
		this.track();
		return Object.entries(this.fields)
			.filter(([, field]) => !field.valid)
			.map(([id]) => ({ id, label: this.#labels[id] }));
	}

	get valid(): boolean {
		return this.missing().length === 0;
	}

	/** Shows every error; returns the id of the first invalid field, or `null` when all are valid. */
	submitAttempt(): string | null {
		let first: string | null = null;
		for (const [id, field] of Object.entries(this.fields)) {
			if (!field.submitAttempt() && first === null) first = id;
		}
		return first;
	}
}

/**
 * The word shown beside a label, or `null`. A form marks only its minority kind: with
 * `marking: 'optional'` optional fields say "(optional)", with `'required'` required ones say
 * "(required)". Public forms always pass `'required'`.
 */
export function markingWord(
	requirement: Marking | undefined,
	marking: Marking | undefined
): Marking | null {
	return requirement && marking && requirement === marking ? requirement : null;
}

export type ReasonPart =
	| { kind: 'text'; text: string }
	| { kind: 'link'; id: string; text: string };

export type Translate = (key: string, values?: Record<string, string | number>) => string;

/** At most this many missing fields are named; the rest are counted. */
export const REASON_NAMES = 3;

/**
 * The reason line shown beside a disabled submit button, as parts so the caller can render the
 * field names as links: "Fill in {A}, {B} and {C} to create the record." With more than three
 * missing fields: "Fill in {A}, {B}, {C} and 2 more to create the record."
 */
export function assembleReason(
	missing: readonly MissingField[],
	action: string,
	t: Translate
): ReasonPart[] {
	if (missing.length === 0) return [];
	const shown = missing.slice(0, REASON_NAMES);
	const more = missing.length - shown.length;
	const parts: ReasonPart[] = [{ kind: 'text', text: t('ui.form.reason.prefix') }];
	shown.forEach((field, index) => {
		if (index > 0) {
			const last = index === shown.length - 1 && more === 0;
			parts.push({
				kind: 'text',
				text: t(last ? 'ui.form.reason.last' : 'ui.form.reason.separator')
			});
		}
		parts.push({ kind: 'link', id: field.id, text: field.label });
	});
	if (more > 0) parts.push({ kind: 'text', text: t('ui.form.reason.more', { n: more }) });
	parts.push({ kind: 'text', text: t('ui.form.reason.suffix', { action }) });
	return parts;
}

/** Moves focus to the control with this id (reason links and the first invalid field). */
export function focusField(id: string): boolean {
	const el = typeof document === 'undefined' ? null : document.getElementById(id);
	if (!el) return false;
	el.focus();
	return true;
}
