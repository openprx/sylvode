import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { User } from '../../src/lib/api/auth';
import { isAdminUser } from '../../src/lib/utils/auth';

const user = (patch: Partial<User>): User =>
	({ id: 'u1', email: 'someone@example.com', ...patch }) as User;

test('an owner or an admin is an instance admin', () => {
	assert.equal(isAdminUser(user({ role: 'owner' })), true);
	assert.equal(isAdminUser(user({ role: 'admin' })), true);
});

test('a member and a signed-out visitor are not', () => {
	assert.equal(isAdminUser(user({ role: 'member' })), false);
	assert.equal(isAdminUser(null), false);
});

test('an e-mail address alone grants nothing', () => {
	assert.equal(isAdminUser(user({ email: 'admin@openpr.local', role: 'member' })), false);
	assert.equal(isAdminUser(user({ email: 'admin@openpr.local' })), false);
});
