const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { activate } = require('../extension.cjs');

test('same-named workspace roots have independent language servers and are removed on close', () => {
  const root = dir => ({ name: 'a-long-workspace-folder-name', uri: { scheme: 'vscode-remote', fsPath: dir, toString: () => `vscode-remote://ssh-remote+fixture${dir}` } });
  const workspace = { isTrusted: true, workspaceFolders: [root('/one/repo'), root('/two/repo')], getConfiguration: () => ({ get: key => key === 'executable' ? '/bin/serena' : key === 'semanticExecutable' ? '/bin/semble' : undefined }) };
  const active = new Map();
  let change;
  workspace.onDidChangeWorkspaceFolders = cb => { change = cb; return {dispose() {}}; };
  const api = { workspace, commands: {registerCommand: () => ({dispose() {}})}, cursor: {mcp: { registerServer({name,server}) {active.set(name,server);}, unregisterServer(name) {active.delete(name);} }} };
  const context = { subscriptions: [], asAbsolutePath: file => path.join('/extension', file) };
  activate(context, api);
  assert.equal(active.size, 4);
  const symbols = [...active.entries()].filter(([id]) => id.startsWith('symbols-'));
  const semantic = [...active.entries()].filter(([id]) => id.startsWith('semantic-'));
  for (const [index, [, server]] of symbols.entries()) {
    assert.equal(server.command, '/bin/serena');
    assert.equal(server.args[server.args.indexOf('--project')+1], workspace.workspaceFolders[index].uri.fsPath);
    assert.equal(server.args[server.args.indexOf('--context')+1], '/extension/symbols.yml');
    assert.equal(server.args[server.args.indexOf('--agent-interface')+1], 'tools');
  }
  for (const [index, [, server]] of semantic.entries()) {
    assert.equal(server.command, process.execPath);
    assert.deepEqual(server.args, ['/extension/semantic.cjs', workspace.workspaceFolders[index].uri.fsPath, '/bin/semble']);
    assert.equal(server.env.ELECTRON_RUN_AS_NODE, '1');
  }
  change({removed:[workspace.workspaceFolders[0]],added:[]});
  assert.equal(active.size,2);
  for (const sub of context.subscriptions) sub.dispose();
  assert.equal(active.size,0);
});

test('remote diagnostics report the effective route rather than a local-file guess', async () => {
  let command, warning;
  const api = {env:{remoteName:'ssh-remote'}, workspace:{isTrusted:true, workspaceFolders:[], onDidChangeWorkspaceFolders:()=>({dispose(){}}), getConfiguration:() => ({get:()=>'never'})}, cursor:{mcp:{registerServer(){}, unregisterServer(){}}}, commands:{registerCommand(_, callback){command=callback;return {dispose(){}};}}, window:{showWarningMessage: msg => {warning=msg;}}};
  activate({subscriptions:[]}, api);
  await command();
  assert.match(warning, /never/);
  assert.match(warning, /always/);
});
