import assert from 'node:assert/strict';
import test from 'node:test';
import { workspaceExtensionBanner } from '../src/features/models/workspaceExtension.ts';

const status = (state, installed_version = null, expected_version = '1.1.0', cursor_cli = true) => ({ state, installed_version, expected_version, cursor_cli });

test('nothing renders without takeover or before the first status fetch', () => {
  assert.deepEqual(workspaceExtensionBanner(status('missing'), false, false), { kind: 'none' });
  assert.deepEqual(workspaceExtensionBanner(status('outdated', '0.9'), false, false), { kind: 'none' });
  assert.deepEqual(workspaceExtensionBanner(null, true, false), { kind: 'none' });
});

test('missing state prompts an install once takeover is on', () => {
  assert.deepEqual(workspaceExtensionBanner(status('missing'), true, false), { kind: 'missing' });
});

test('outdated state carries installed and expected versions', () => {
  assert.deepEqual(workspaceExtensionBanner(status('outdated', '0.9.0'), true, false),
    { kind: 'outdated', installed: '0.9.0', expected: '1.1.0' });
  assert.deepEqual(workspaceExtensionBanner(status('outdated'), true, false),
    { kind: 'outdated', installed: null, expected: '1.1.0' });
});

test('dismissed hides the prompt for the session but never the installed line', () => {
  assert.deepEqual(workspaceExtensionBanner(status('missing'), true, true), { kind: 'none' });
  assert.deepEqual(workspaceExtensionBanner(status('outdated', '0.9'), true, true), { kind: 'none' });
  assert.deepEqual(workspaceExtensionBanner(status('current', '1.1.0'), true, true), { kind: 'current', version: '1.1.0' });
});

test('current state always reports the installed version', () => {
  assert.deepEqual(workspaceExtensionBanner(status('current', '1.1.0'), true, false), { kind: 'current', version: '1.1.0' });
  assert.deepEqual(workspaceExtensionBanner(status('current', null, '1.1.0'), true, false), { kind: 'current', version: '1.1.0' });
});
