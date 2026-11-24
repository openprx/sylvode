// The single place that turns a person (or a missing one) into display text. The server sends a
// person as `{id, name, is_bot, state}` and never an id where a name belongs; the client words the
// cases without a name itself: a person who has left ("Removed user") and a person it knows
// nothing about ("Someone"). Nothing here ever returns an id, not even a name that looks like one.

/** A translation function as `svelte-i18n`'s `$t` has it. */
export type Translate = (key: string, options?: { values?: Record<string, unknown> }) => string;

/** A person as the API shows them next to a comment, a mention or an activity entry. */
export interface Person {
	id: string;
	name: string | null;
	is_bot: boolean;
	state: 'active' | 'removed';
}

/** People by user id, as `data.people` of the activity answers carries them. */
export type PeopleById = Readonly<Record<string, Person | undefined>>;

export interface PersonDisplay {
	/** The text to show for the person, without any badge. */
	name: string;
	isBot: boolean;
	/** The name is the stand-in for a person who has left (no last known name). */
	removed: boolean;
	/** The name is the neutral stand-in: nothing is known about the person. */
	unknown: boolean;
}

const UUID_TEXT = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i;

/** A usable name: non-blank, and not an id that slipped into a name field. */
export function usableName(value: unknown): string | null {
	if (typeof value !== 'string') return null;
	const text = value.trim();
	if (!text || UUID_TEXT.test(text)) return null;
	return text;
}

/** The text for a person nothing is known about. */
export function someoneText(t: Translate): string {
	return t('people.someone');
}

/** The text for a person who has left and left no name. */
export function removedUserText(t: Translate): string {
	return t('people.removedUser');
}

/** What to show for a person; `null`/`undefined` is a person the client has no information on. */
export function personDisplay(person: Person | null | undefined, t: Translate): PersonDisplay {
	if (!person) {
		return { name: someoneText(t), isBot: false, removed: false, unknown: true };
	}
	const name = usableName(person.name);
	if (name) {
		return { name, isBot: person.is_bot === true, removed: false, unknown: false };
	}
	if (person.state === 'removed') {
		return {
			name: removedUserText(t),
			isBot: person.is_bot === true,
			removed: true,
			unknown: false
		};
	}
	return { name: someoneText(t), isBot: person.is_bot === true, removed: false, unknown: true };
}

/** The person's name for a line of text: the badge text is added for a bot ("Name (Bot)"). */
export function personLabel(person: Person | null | undefined, t: Translate): string {
	const shown = personDisplay(person, t);
	return shown.isBot ? t('people.botLabel', { values: { name: shown.name } }) : shown.name;
}

/** The person with this id from a `people` object, shown as `personLabel` does. */
export function personLabelById(
	people: PeopleById | null | undefined,
	id: string | null | undefined,
	t: Translate
): string {
	return personLabel(id ? people?.[id] : null, t);
}

/** A name from a list of people who have a name and an e-mail (members): never an id. */
export function directoryName(
	entry: { name?: string | null; email?: string | null } | null | undefined,
	t: Translate
): string {
	return usableName(entry?.name) ?? usableName(entry?.email) ?? someoneText(t);
}

/** A name that may be empty (a notification's actor, a Flow candidate): the neutral text then. */
export function nameOrSomeone(name: string | null | undefined, t: Translate): string {
	return usableName(name) ?? someoneText(t);
}

/** The people that arrive in an answer, with the fields the contract promises and no others. */
export function readPerson(raw: unknown): Person | null {
	if (!raw || typeof raw !== 'object') return null;
	const value = raw as Record<string, unknown>;
	if (typeof value.id !== 'string') return null;
	return {
		id: value.id,
		name: typeof value.name === 'string' ? value.name : null,
		is_bot: value.is_bot === true,
		state: value.state === 'removed' ? 'removed' : 'active'
	};
}

export function readPeople(raw: unknown): Person[] {
	return Array.isArray(raw) ? raw.flatMap((item) => readPerson(item) ?? []) : [];
}

/** The `people` object of an activity answer, keyed by user id. */
export function readPeopleById(raw: unknown): Record<string, Person> {
	const result: Record<string, Person> = {};
	if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return result;
	for (const [id, value] of Object.entries(raw as Record<string, unknown>)) {
		const person = readPerson({ ...(value as object), id });
		if (person) result[id] = person;
	}
	return result;
}
