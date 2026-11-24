/**
 * The one place that turns a person into display text: names, the Bot badge, "Removed user",
 * "Someone", and never an id. Also the wire readers for the people the API sends.
 *
 * Run standalone: `bun test tests/features/person-label.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
	directoryName,
	nameOrSomeone,
	personDisplay,
	personLabel,
	personLabelById,
	readMentioned,
	readPeopleById,
	readPerson,
	usableName,
	type Person
} from '../../src/lib/people/display';
import { randomUuid } from '../../src/lib/utils/uuid';

function translator(language: 'en' | 'zh') {
	const raw = JSON.parse(
		readFileSync(new URL(`../../src/lib/people/locale.${language}.json`, import.meta.url), 'utf8')
	) as Record<string, unknown>;
	return (key: string, options?: { values?: Record<string, unknown> }): string => {
		let node: unknown = raw;
		for (const part of key.split('.')) node = (node as Record<string, unknown> | undefined)?.[part];
		assert.equal(typeof node, 'string', `missing text ${key}`);
		return (node as string).replace(/\{(\w+)\}/g, (_, name) => String(options?.values?.[name]));
	};
}

const UUID_ANYWHERE = /[0-9a-f]{8}-/i;
const person = (over: Partial<Person> = {}): Person => ({
	id: randomUuid(),
	name: 'Ada',
	is_bot: false,
	state: 'active',
	...over
});

test('a person is shown by name, a bot with its badge', () => {
	for (const language of ['en', 'zh'] as const) {
		const t = translator(language);
		assert.equal(personLabel(person(), t), 'Ada');
		const bot = personLabel(person({ name: 'Builder', is_bot: true }), t);
		assert.ok(bot.includes('Builder'));
		assert.ok(bot.includes(t('people.bot')));
		assert.equal(personDisplay(person({ is_bot: true }), t).isBot, true);
	}
});

test('a person who has left shows the last known name, else the removed-user text', () => {
	const en = translator('en');
	const zh = translator('zh');
	assert.equal(personLabel(person({ state: 'removed', name: 'Grace' }), en), 'Grace');
	assert.equal(personLabel(person({ state: 'removed', name: null }), en), 'Removed user');
	assert.equal(personLabel(person({ state: 'removed', name: null }), zh), '已移除的用户');
	assert.equal(personDisplay(person({ state: 'removed', name: null }), en).removed, true);
});

test('a missing person, or an active one without a name, is "Someone"', () => {
	const en = translator('en');
	const zh = translator('zh');
	assert.equal(personLabel(null, en), 'Someone');
	assert.equal(personLabel(undefined, zh), '某人');
	assert.equal(personLabel(person({ name: null }), en), 'Someone');
	assert.equal(personLabel(person({ name: '   ' }), en), 'Someone');
	assert.equal(personDisplay(null, en).unknown, true);
	assert.equal(personLabelById({}, randomUuid(), en), 'Someone');
	assert.equal(personLabelById(undefined, null, en), 'Someone');
});

test('the helpers never return an id, whatever they are given', () => {
	const id = randomUuid();
	const prefix = id.slice(0, 8);
	const fixtures: Array<Person | null | undefined> = [
		null,
		undefined,
		person({ name: null }),
		person({ name: id }),
		person({ name: ` ${id} `, is_bot: true }),
		person({ name: id, state: 'removed' }),
		person({ name: null, state: 'removed', is_bot: true })
	];
	for (const language of ['en', 'zh'] as const) {
		const t = translator(language);
		for (const fixture of fixtures) {
			for (const text of [personLabel(fixture, t), personDisplay(fixture, t).name]) {
				assert.ok(!UUID_ANYWHERE.test(text), `id in "${text}"`);
				assert.ok(!text.includes(prefix), `id prefix in "${text}"`);
			}
		}
		for (const text of [
			nameOrSomeone(id, t),
			nameOrSomeone('', t),
			nameOrSomeone(null, t),
			directoryName({ name: '', email: id }, t),
			directoryName({ name: id }, t),
			directoryName(null, t),
			personLabelById({ [id]: person({ id, name: id }) }, id, t)
		]) {
			assert.ok(!UUID_ANYWHERE.test(text), `id in "${text}"`);
		}
	}
	assert.equal(usableName(id), null);
	assert.equal(usableName('Ada Lovelace'), 'Ada Lovelace');
});

test('a member without a name is shown by e-mail, then as "Someone"', () => {
	const t = translator('en');
	assert.equal(directoryName({ name: 'Ada', email: 'ada@example.com' }, t), 'Ada');
	assert.equal(directoryName({ name: '', email: 'ada@example.com' }, t), 'ada@example.com');
	assert.equal(directoryName({ name: '', email: '' }, t), 'Someone');
	assert.equal(nameOrSomeone('', t), 'Someone');
});

test('the readers keep only what the contract promises', () => {
	const id = randomUuid();
	assert.deepEqual(readPerson({ id, name: 'Ada', is_bot: true, state: 'removed', extra: 1 }), {
		id,
		name: 'Ada',
		is_bot: true,
		state: 'removed'
	});
	assert.deepEqual(readPerson({ id, name: null }), {
		id,
		name: null,
		is_bot: false,
		state: 'active'
	});
	assert.equal(readPerson(null), null);
	assert.equal(readPerson({ name: 'no id' }), null);
	assert.deepEqual(readMentioned(undefined), []);
	assert.equal(readMentioned([{ id, name: 'A' }, 5, null]).length, 1);
	const byId = readPeopleById({ [id]: { name: 'Ada', is_bot: false, state: 'active' } });
	assert.equal(byId[id]?.id, id);
	assert.equal(byId[id]?.name, 'Ada');
	assert.deepEqual(readPeopleById(null), {});
});

test('both locales carry the same people texts', () => {
	const read = (language: string) =>
		JSON.parse(
			readFileSync(new URL(`../../src/lib/people/locale.${language}.json`, import.meta.url), 'utf8')
		) as { people: Record<string, string> };
	assert.deepEqual(Object.keys(read('en').people).sort(), Object.keys(read('zh').people).sort());
	assert.equal(read('zh').people.removedUser, '已移除的用户');
	assert.equal(read('zh').people.someone, '某人');
});
