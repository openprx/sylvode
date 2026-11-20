import { createRawSnippet } from 'svelte';
import {
	Suite,
	assert,
	assertDeepEqual,
	assertEqual,
	finish
} from '../../../../tests/support/harness';
import { installSvelteSsr, renderToHtml } from '../../../../tests/support/svelte-ssr';
import { FieldState, FormState, assembleReason, markingWord, type Translate } from './field-state';

await installSvelteSsr();
const { default: Field } = await import('./Field.svelte');
const { default: SubmitReason } = await import('./SubmitReason.svelte');

const suite = new Suite('ui-form');
const email = (v: string) =>
	/^\S+@\S+\.\S+$/.test(v) ? null : 'Enter an email address like name@example.com.';

suite.check('no error appears while typing the first time', () => {
	const field = new FieldState<string>('', email);
	field.input('na');
	field.input('name@');
	assertEqual(field.shownError, null, 'nothing shown');
	assertEqual(field.valid, false, 'still invalid underneath');
});
suite.check('the error appears on blur and clears on the input that makes the value valid', () => {
	const field = new FieldState<string>('', email);
	field.input('name@');
	field.blur();
	assertEqual(field.shownError, 'Enter an email address like name@example.com.', 'shown on blur');
	field.input('name@example');
	assert(field.shownError !== null, 'still shown while invalid');
	field.input('name@example.com');
	assertEqual(field.shownError, null, 'cleared without a blur');
	field.input('name@');
	assert(field.shownError !== null, 'shown again as the user types once it has been shown');
});
suite.check('blur validates an edited field, or a required one that was only visited', () => {
	const untouched = new FieldState<string>('', email);
	untouched.blur();
	assertEqual(untouched.shownError, null, 'optional and unedited stays quiet');
	const required = new FieldState<string>('', email, { required: true });
	required.blur();
	assert(required.shownError !== null, 'required and visited is validated');
});
suite.check('submit attempt shows the error and reports validity', () => {
	const field = new FieldState<string>('x', email);
	assertEqual(field.submitAttempt(), false, 'invalid');
	assert(field.shownError !== null, 'shown');
	field.input('a@b.co');
	assertEqual(field.submitAttempt(), true, 'valid');
});
suite.check('a server error is shown and the typed value is kept', () => {
	const field = new FieldState<string>('taken@example.com', email);
	field.setServerError('That address is already in use.');
	assertEqual(field.shownError, 'That address is already in use.', 'message');
	assertEqual(field.value, 'taken@example.com', 'value kept');
});
suite.check('FormState lists missing fields in order and reports the first invalid id', () => {
	const form = new FormState('required');
	form.register('title', 'Title', new FieldState('', (v) => (v ? null : 'x')));
	form.register('due', 'Due date', new FieldState('', (v) => (v ? null : 'x')));
	form.register('owner', 'Owner', new FieldState('me', (v) => (v ? null : 'x')));
	assertDeepEqual(
		form.missing(),
		[
			{ id: 'title', label: 'Title' },
			{ id: 'due', label: 'Due date' }
		],
		'missing'
	);
	assertEqual(form.valid, false, 'not valid');
	assertEqual(form.submitAttempt(), 'title', 'first invalid');
	assert(form.fields.due.shownError !== null, 'every error is shown after a submit attempt');
	form.fields.title.input('t');
	form.fields.due.input('d');
	assertEqual(form.valid, true, 'valid');
	assertEqual(form.submitAttempt(), null, 'nothing to focus');
});

suite.check('marking word: only the minority kind is marked', () => {
	assertEqual(markingWord('required', 'required'), 'required', 'required form marks required');
	assertEqual(markingWord('optional', 'required'), null, 'and leaves optional bare');
	assertEqual(markingWord('optional', 'optional'), 'optional', 'optional form marks optional');
	assertEqual(markingWord('required', 'optional'), null, 'and leaves required bare');
	assertEqual(markingWord(undefined, 'required'), null, 'no requirement, no word');
	assertEqual(markingWord('required', undefined), null, 'no marking, no word');
});

const dict: Record<string, string> = {
	'ui.form.reason.prefix': 'Fill in ',
	'ui.form.reason.separator': ', ',
	'ui.form.reason.last': ' and ',
	'ui.form.reason.more': ' and {n} more',
	'ui.form.reason.suffix': ' to {action}.'
};
const translate: Translate = (key, values) =>
	(dict[key] ?? key).replace(/\{(\w+)\}/g, (_, k: string) => String(values?.[k]));
const text = (parts: ReturnType<typeof assembleReason>) => parts.map((p) => p.text).join('');
const names = (n: number) =>
	Array.from({ length: n }, (_, i) => ({ id: `f${i}`, label: `Field ${i + 1}` }));

suite.check('reason line: one, two and three names', () => {
	assertEqual(
		text(assembleReason(names(1), 'create the record', translate)),
		'Fill in Field 1 to create the record.',
		'one'
	);
	assertEqual(
		text(assembleReason(names(2), 'create the record', translate)),
		'Fill in Field 1 and Field 2 to create the record.',
		'two'
	);
	assertEqual(
		text(assembleReason(names(3), 'save', translate)),
		'Fill in Field 1, Field 2 and Field 3 to save.',
		'three'
	);
});
suite.check('reason line: more than three names are counted, and names are links', () => {
	const parts = assembleReason(names(5), 'save', translate);
	assertEqual(text(parts), 'Fill in Field 1, Field 2, Field 3 and 2 more to save.', 'five');
	assertEqual(parts.filter((p) => p.kind === 'link').length, 3, 'three links');
	assertDeepEqual(
		parts.filter((p) => p.kind === 'link').map((p) => (p.kind === 'link' ? p.id : '')),
		['f0', 'f1', 'f2'],
		'link ids'
	);
	assertDeepEqual(assembleReason([], 'save', translate), [], 'nothing missing, no line');
});

const body = createRawSnippet<
	[{ describedBy: string | undefined; invalid: boolean; required: boolean }]
>((state) => ({
	render: () =>
		`<input id="f" aria-required="${state().required}" aria-invalid="${state().invalid}" aria-describedby="${state().describedBy ?? ''}">`
}));

await suite.checkAsync('Field: the marking word follows the form marking', async () => {
	const required = await renderToHtml(Field, {
		id: 'f',
		label: 'Title',
		requirement: 'required',
		marking: 'required',
		children: body
	});
	assert(required.includes('ui.form.required'), required);
	const optional = await renderToHtml(Field, {
		id: 'f',
		label: 'Title',
		requirement: 'optional',
		marking: 'required',
		children: body
	});
	assert(!optional.includes('ui.form.'), optional);
	const flipped = await renderToHtml(Field, {
		id: 'f',
		label: 'Title',
		requirement: 'optional',
		marking: 'optional',
		children: body
	});
	assert(flipped.includes('ui.form.optional'), flipped);
});
await suite.checkAsync(
	'Field: aria-required is set for required fields whatever the marking',
	async () => {
		const html = await renderToHtml(Field, {
			id: 'f',
			label: 'Title',
			requirement: 'required',
			marking: 'optional',
			children: body
		});
		assert(html.includes('aria-required="true"'), html);
		assert(!html.includes('ui.form.'), 'no word, still required for assistive technology');
	}
);
await suite.checkAsync(
	'Field: an error replaces the helper, is live and wired to the control',
	async () => {
		const html = await renderToHtml(Field, {
			id: 'f',
			label: 'Title',
			helper: 'Up to 200 characters',
			error: 'Enter a title',
			children: body
		});
		assert(html.includes('aria-live="polite"'), 'live region');
		assert(html.includes('Enter a title') && !html.includes('Up to 200'), 'error replaces helper');
		assert(
			html.includes('aria-invalid="true"') && html.includes('aria-describedby="f-message"'),
			html
		);
		assert(html.includes('<svg'), 'alert glyph');
	}
);
await suite.checkAsync(
	'SubmitReason renders at most three links and nothing when valid',
	async () => {
		const html = await renderToHtml(SubmitReason, { missing: names(5), action: 'save' });
		assertEqual((html.match(/<button/g) ?? []).length, 3, 'links');
		assert(html.includes('Field 1') && !html.includes('Field 4'), 'names');
		assertEqual(
			(await renderToHtml(SubmitReason, { missing: [], action: 'save' })).includes('submit-reason'),
			false,
			'hidden when valid'
		);
	}
);

finish(suite.result());
