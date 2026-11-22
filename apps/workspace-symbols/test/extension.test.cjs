const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { activate } = require('../extension.cjs');
const { spawnRelay, responses } = require('./fakes.cjs');

const root = dir => ({ name: 'a-long-workspace-folder-name', uri: { scheme: 'vscode-remote', fsPath: dir, toString: () => `vscode-remote://ssh-remote+fixture${dir}` } });

function setup(t, folders) {
  const storage = fs.mkdtempSync(path.join(os.tmpdir(), 'ext-storage-'));
  t.after(() => fs.rmSync(storage, { recursive: true, force: true }));
  const workspace = {
    isTrusted: true,
    workspaceFolders: folders,
    getConfiguration: () => ({ get: key => key === 'semanticExecutable' ? '/bin/byok-semantic' : undefined }),
  };
  let change;
  workspace.onDidChangeWorkspaceFolders = cb => { change = cb; return { dispose() {} }; };
  const active = new Map();
  const api = {
    workspace,
    commands: { registerCommand: () => ({ dispose() {} }), executeCommand: async () => undefined },
    languages: { getLanguages: async () => [], getLanguageConfiguration: () => ({}) },
    cursor: { mcp: {
      registerServer({ name, server }) { active.set(name, server); },
      unregisterServer(name) { active.delete(name); },
    } },
  };
  const context = {
    subscriptions: [],
    globalStorageUri: { fsPath: storage, scheme: 'file', toString: () => `file://${storage}` },
    asAbsolutePath: file => path.join(__dirname, '..', file),
  };
  return { workspace, active, api, context, storage, onFoldersChange: folders => change(folders) };
}

test('each workspace root registers a relayed symbols server and a byok-semantic server', async (t) => {
  const { workspace, active, api, context, storage, onFoldersChange } = setup(t, [root('/one/repo'), root('/two/repo')]);
  const activation = await activate(context, api);
  await activation.ready;
  assert.equal(active.size, 4);
  const symbols = [...active.entries()].filter(([id]) => id.startsWith('symbols-'));
  const semantic = [...active.entries()].filter(([id]) => id.startsWith('semantic-'));
  for (const [index, [, server]] of symbols.entries()) {
    // The relay runs the extension-bundled script under ELECTRON_RUN_AS_NODE so the
    // Electron binary behaves like plain node; the socket path reaches it via env.
    assert.equal(server.command, process.execPath);
    assert.deepEqual(server.args, [path.join(__dirname, '..', 'relay.cjs')]);
    assert.equal(server.env.ELECTRON_RUN_AS_NODE, '1');
    const socketPath = server.env.BYOK_SYMBOLS_SOCKET;
    // macOS sun_path is 104 bytes; the socket must stay short regardless of the
    // extension's storage path, so it lives in a private tmpdir, not globalStorage.
    assert.ok(Buffer.byteLength(socketPath) < 104, socketPath);
    assert.ok(fs.existsSync(socketPath), 'socket file exists');
    assert.equal(path.basename(path.dirname(socketPath)).slice(0, 5), 'byok-');
    assert.equal(fs.statSync(path.dirname(socketPath)).mode & 0o777, 0o700);
  }
  for (const [index, [, server]] of semantic.entries()) {
    // The semantic server is the byok-semantic binary itself; the only argument
    // is the workspace root it binds its repo to.
    assert.equal(server.command, '/bin/byok-semantic');
    assert.deepEqual(server.args, [workspace.workspaceFolders[index].uri.fsPath]);
    assert.deepEqual(server.env ?? {}, {});
  }
  // Removing a workspace root unregisters both servers and deletes the socket file.
  const removedSocket = symbols[0][1].env.BYOK_SYMBOLS_SOCKET;
  onFoldersChange({ removed: [workspace.workspaceFolders[0]], added: [] });
  assert.equal(active.size, 2);
  assert.ok(!fs.existsSync(removedSocket), 'socket file removed with the folder');
  for (const sub of context.subscriptions) sub.dispose();
  assert.equal(active.size, 0);
});

test('sockets stay under the system sun_path limit however deep globalStorage is', async (t) => {
  // 130+ bytes of globalStorage would push a socket path past macOS's 104-byte
  // sun_path limit; the regression that put sockets there broke every symbols
  // server with EINVAL.
  const deep = path.join(t.name ? fs.mkdtempSync(path.join(os.tmpdir(), 'ext-deep-')) : os.tmpdir(), 'very', 'deep', 'storage', 'path', 'that', 'is', 'way', 'past', 'one', 'hundred', 'and', 'four', 'bytes', 'total', 'abc');
  fs.mkdirSync(deep, { recursive: true });
  t.after(() => fs.rmSync(deep.slice(0, deep.indexOf(`${path.sep}very`)), { recursive: true, force: true }));
  const { api, context, active } = setup(t, [root('/deep/repo')]);
  context.globalStorageUri = { fsPath: deep, scheme: 'file', toString: () => `file://${deep}` };
  const activation = await activate(context, api);
  await activation.ready;
  t.after(() => { for (const sub of context.subscriptions) sub.dispose(); });
  const socketPath = [...active.entries()].find(([id]) => id.startsWith('symbols-'))[1].env.BYOK_SYMBOLS_SOCKET;
  assert.ok(Buffer.byteLength(socketPath) < 104, socketPath);
  assert.equal(fs.statSync(path.dirname(socketPath)).mode & 0o777, 0o700);
  const relay = spawnRelay(socketPath);
  t.after(() => relay.kill());
  const next = responses(relay.stdout);
  relay.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: {} })}\n`);
  assert.equal((await next()).result.protocolVersion, '2025-06-18');
});

test('remote diagnostics report the effective route rather than a local-file guess', async (t) => {
  const { api, context } = setup(t, []);
  api.env = { remoteName: 'ssh-remote' };
  api.workspace.getConfiguration = () => ({ get: () => 'never' });
  let command, warning;
  api.commands.registerCommand = (_, callback) => { command = callback; return { dispose() {} }; };
  api.window = { showWarningMessage: msg => { warning = msg; } };
  await activate(context, api);
  t.after(() => { for (const sub of context.subscriptions) sub.dispose(); });
  await command();
  assert.match(warning, /never/);
  assert.match(warning, /always/);
});
