const assert = require('node:assert/strict');
const test = require('node:test');
const { bindRequest, bindResponse } = require('../semantic.cjs');

test('model supplied paths and URLs cannot move the semantic index outside the workspace', () => {
  const request = { method: 'tools/call', params: { name: 'search', arguments: { query: 'apply rules', repo: 'https://example.test/repo' } } };
  assert.deepEqual(bindRequest(request, '/workspace/app').params.arguments, { query: 'apply rules', repo: '/workspace/app' });
  const related = { method: 'tools/call', params: { name: 'find_related', arguments: { file_path: 'src/index.ts', line: 45 } } };
  assert.equal(bindRequest(related, '/workspace/app').params.arguments.repo, '/workspace/app');
});

test('the published semantic tool schema has no repository argument', () => {
  const tool = { name: 'search', description: 'Index any repository', inputSchema: { properties: { repo: {}, query: {} }, required: ['query', 'repo'] } };
  const reply = bindResponse({ result: { tools: [tool] } });
  assert.deepEqual(reply.result.tools[0].inputSchema, { properties: { query: {} }, required: ['query'] });
});

test('indexing failures have MCP error status independent of empty results', () => {
  const result = text => bindResponse({ result: { content: [{ type: 'text', text }], isError: false } }).result;
  assert.equal(result('Failed to index /workspace/app: permissions').isError, true);
  assert.equal(result('{"error":"No results found."}').isError, false);
});
