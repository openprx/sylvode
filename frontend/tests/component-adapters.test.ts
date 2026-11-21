import { createRawSnippet } from 'svelte';
import { Suite, assert, assertEqual, finish } from './support/harness';
import { installSvelteSsr, renderToHtml } from './support/svelte-ssr';

await installSvelteSsr();
const comp = (name: string) => import(`../src/lib/components/${name}.svelte`);
const text = createRawSnippet(() => ({ render: () => '<span>Label</span>' }));
const suite = new Suite('component-adapters');

// Button
const buttonModule = await comp('Button');
suite.check('Button variants map onto the primitive', () => {
	const map = buttonModule.mapButtonVariant as (v?: string) => string;
	assertEqual(map('primary'), 'primary', 'primary');
	assertEqual(map('secondary'), 'secondary', 'secondary');
	assertEqual(map('ghost'), 'text', 'ghost');
	assertEqual(map('danger'), 'danger-quiet', 'danger');
	assertEqual(map('dangerSolid'), 'danger', 'dangerSolid');
	assertEqual(map(undefined), 'primary', 'default');
	assertEqual(map('nope'), 'secondary', 'unknown');
});
await suite.checkAsync('Button keeps type, size, aria-expanded, disabled and loading', async () => {
	const html = await renderToHtml(buttonModule.default, {
		variant: 'dangerSolid',
		size: 'sm',
		type: 'submit',
		'aria-expanded': true,
		disabled: true,
		children: text
	});
	assert(html.includes('v-danger') && !html.includes('danger-quiet'), html);
	assert(html.includes('size-sm') && html.includes('type="submit"'), html);
	assert(html.includes('aria-expanded="true"') && /\sdisabled/.test(html), html);
	const busy = await renderToHtml(buttonModule.default, { loading: true, children: text });
	assert(busy.includes('aria-busy="true"') && busy.includes('loading'), busy);
});
await suite.checkAsync('Button without children renders', async () => {
	const html = await renderToHtml(buttonModule.default, {});
	assert(html.includes('data-ui="button"'), html);
});

// Input / Textarea
await suite.checkAsync('Input maps label, required, helper, error, id and type', async () => {
	const { default: Input } = await comp('Input');
	const html = await renderToHtml(Input, {
		label: 'Email',
		required: true,
		helperText: 'We never share it',
		type: 'email',
		id: 'email-field',
		value: 'a@b.c'
	});
	assert(html.includes('for="email-field"') && html.includes('id="email-field"'), html);
	assert(html.includes('type="email"') && /\srequired/.test(html), html);
	assert(html.includes('ui.form.required'), 'required marking word');
	assert(html.includes('We never share it'), html);
	const bad = await renderToHtml(Input, { label: 'Email', error: 'Bad', hint: 'ignored' });
	assert(
		bad.includes('Bad') && bad.includes('aria-invalid="true"') && !bad.includes('ignored'),
		bad
	);
});
suite.check('Input without a label falls back to the placeholder as a hidden label', async () => {
	const { inputLabel } = await comp('Input');
	assertEqual(JSON.stringify(inputLabel('L', 'p', 'n')), '{"text":"L","hidden":false}', 'label');
	assertEqual(JSON.stringify(inputLabel(undefined, 'p', 'n')), '{"text":"p","hidden":true}', 'ph');
	assertEqual(
		JSON.stringify(inputLabel(undefined, undefined, 'n')),
		'{"text":"n","hidden":true}',
		'n'
	);
});
await suite.checkAsync('Textarea keeps rows, maxlength and label', async () => {
	const { default: Textarea } = await comp('Textarea');
	const html = await renderToHtml(Textarea, { label: 'Notes', rows: 7, maxlength: 20, value: 'x' });
	assert(
		html.includes('Notes') && html.includes('rows="7"') && html.includes('maxlength="20"'),
		html
	);
});

// Select
suite.check('Select maps the empty string to null unless an option has it', async () => {
	const { toPrimitiveValue, fromPrimitiveValue } = await comp('Select');
	const opts = [{ label: 'A', value: 'a' }];
	assertEqual(toPrimitiveValue('', opts), null, 'empty -> null');
	assertEqual(toPrimitiveValue('a', opts), 'a', 'value kept');
	assertEqual(toPrimitiveValue('', [{ label: 'None', value: '' }]), '', 'empty option kept');
	assertEqual(fromPrimitiveValue(null), '', 'null -> empty');
});
await suite.checkAsync('Select renders its label', async () => {
	const { default: Select } = await comp('Select');
	const html = await renderToHtml(Select, {
		label: 'Status',
		value: 'b',
		options: [
			{ label: 'A', value: 'a' },
			{ label: 'B', value: 'b' }
		]
	});
	assert(html.includes('Status') && html.includes('B'), html);
});

// Badge
await suite.checkAsync('Badge maps priority and status to tones, words stay', async () => {
	const { badgeTone, default: Badge } = await comp('Badge');
	assertEqual(badgeTone('priority', 'urgent'), 'danger', 'urgent');
	assertEqual(badgeTone('priority', 'high'), 'warning', 'high');
	assertEqual(badgeTone('priority', 'low'), 'neutral', 'low');
	assertEqual(badgeTone('status', 'done'), 'success', 'done');
	assertEqual(badgeTone('status', 'in_progress'), 'neutral', 'in progress');
	assertEqual(badgeTone('status', 'custom'), 'neutral', 'unknown');
	const html = await renderToHtml(Badge, { value: 'urgent', kind: 'priority' });
	assert(html.includes('data-tone="danger"') && html.includes('issue.urgent'), html);
	const raw = await renderToHtml(Badge, { value: 'custom' });
	assert(raw.includes('>custom<'), raw);
});

// Tag
await suite.checkAsync('Tag shows the label and a remove button only when removable', async () => {
	const { default: Tag } = await comp('Tag');
	const plain = await renderToHtml(Tag, { label: 'bug', color: '#ff0000' });
	assert(plain.includes('bug') && plain.includes('#ff0000') && !plain.includes('<button'), plain);
	const removable = await renderToHtml(Tag, { label: 'bug', removable: true });
	assert(removable.includes('<button') && removable.includes('activity.labelRemoved'), removable);
});

// Card
await suite.checkAsync('Card renders a button only when clickable with a handler', async () => {
	const { default: Card } = await comp('Card');
	const div = await renderToHtml(Card, { children: text });
	assert(div.includes('<div class="ui-card pad-md') && !div.includes('<button'), div);
	const none = await renderToHtml(Card, { padding: 'none', children: text });
	assert(!none.includes('pad-'), none);
	const button = await renderToHtml(Card, { clickable: true, onclick: () => {}, children: text });
	assert(button.includes('<button') && button.includes('clickable'), button);
	const noHandler = await renderToHtml(Card, { clickable: true, children: text });
	assert(!noHandler.includes('<button'), noHandler);
});

// EmptyState
await suite.checkAsync('EmptyState keeps title, description and action slot', async () => {
	const { default: EmptyState } = await comp('EmptyState');
	const html = await renderToHtml(EmptyState, {
		icon: 'office',
		title: 'No projects',
		description: 'Create one',
		action: text
	});
	assert(
		html.includes('No projects') && html.includes('Create one') && html.includes('Label'),
		html
	);
	for (const icon of ['clipboard', 'box', 'search', 'office']) {
		const out = await renderToHtml(EmptyState, { icon, title: 'x' });
		assert(out.includes('<svg'), `icon ${icon}`);
	}
	const bare = await renderToHtml(EmptyState, { title: 'x' });
	assert(!bare.includes('class="body') && !bare.includes('class="action'), bare);
});

// Avatar
suite.check('Avatar names itself from name, then user id', async () => {
	const { avatarName } = await comp('Avatar');
	assertEqual(avatarName(' Ada Lovelace ', 'u1'), 'Ada Lovelace', 'name');
	assertEqual(avatarName(undefined, ' u1 '), 'u1', 'user id');
	assertEqual(avatarName(), '', 'none');
});
await suite.checkAsync('Avatar passes size and a title', async () => {
	const { default: Avatar } = await comp('Avatar');
	const html = await renderToHtml(Avatar, { name: 'Ada Lovelace', size: 'md' });
	assert(
		html.includes('size-md') && html.includes('title="Ada Lovelace"') && html.includes('>AL<'),
		html
	);
	const none = await renderToHtml(Avatar, {});
	assert(none.includes('>?<') && none.includes('common.unassigned'), none);
});

// ProgressBar
await suite.checkAsync('ProgressBar keeps value, label and the percentage switch', async () => {
	const { default: ProgressBar } = await comp('ProgressBar');
	const html = await renderToHtml(ProgressBar, { value: 42.4, label: 'Import' });
	assert(
		html.includes('Import') && html.includes('aria-valuenow="42"') && html.includes('42%'),
		html
	);
	const defaults = await renderToHtml(ProgressBar, { value: 10 });
	assert(defaults.includes('common.status'), defaults);
	const hidden = await renderToHtml(ProgressBar, { value: 10, showPercentage: false });
	assert(!hidden.includes('>10%<') && !hidden.includes('common.status'), hidden);
});

// Modal
suite.check('Modal widths: up to max-w-lg is the 480 dialog, wider is the 680 one', async () => {
	const { dialogSize } = await comp('Modal');
	for (const c of ['max-w-sm', 'max-w-md', 'max-w-lg']) assertEqual(dialogSize(c), 'md', c);
	for (const c of ['max-w-2xl', 'max-w-3xl', 'max-w-4xl']) assertEqual(dialogSize(c), 'wide', c);
});
await suite.checkAsync('Modal renders nothing closed, a labelled dialog open', async () => {
	const { default: Modal } = await comp('Modal');
	const closed = await renderToHtml(Modal, { open: false, title: 'T', children: text });
	assert(!closed.includes('role="dialog"'), closed);
	const open = await renderToHtml(Modal, {
		open: true,
		title: 'Create thing',
		children: text,
		footer: text
	});
	assert(open.includes('role="dialog"') && open.includes('Create thing'), open);
});

// Toast
await suite.checkAsync('Toast renders the new region', async () => {
	const { default: Toast } = await comp('Toast');
	const html = await renderToHtml(Toast, {});
	assert(html.includes('data-ui="toast-region"'), html);
});

finish(suite.result());
