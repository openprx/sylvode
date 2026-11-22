import assert from 'node:assert/strict';
import { test } from 'node:test';
import { codeChallengeS256, isValidVerifier, newState, newVerifier } from '../lib/pkce.ts';

test('the S256 challenge matches the RFC 7636 appendix B vector', () => {
	assert.equal(
		codeChallengeS256('dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk'),
		'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM'
	);
});

test('generated verifiers and states are well formed and differ', () => {
	const verifier = newVerifier();
	assert.equal(verifier.length, 43);
	assert.ok(isValidVerifier(verifier));
	assert.notEqual(verifier, newVerifier());
	const state = newState();
	assert.ok(state.length >= 16 && /^[A-Za-z0-9_-]+$/.test(state));
	assert.equal(isValidVerifier('short'), false);
});
