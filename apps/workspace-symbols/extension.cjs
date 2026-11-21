//! Registers workspace-local semantic search and language-server MCP entry points.
const { createHash } = require('node:crypto');

function activate(context, vscode = require('vscode')) {
  const { workspace } = vscode;
  if (!workspace.isTrusted) return;
  context.subscriptions.push(vscode.commands.registerCommand('byok.checkInferenceRoute', () => {
    if (!vscode.env.remoteName) return vscode.window.showInformationMessage('Local workspace: remoteInferenceRoute does not apply.');
    const scope = vscode.window.activeTextEditor?.document.uri ?? workspace.workspaceFolders?.[0]?.uri;
    const route = workspace.getConfiguration('cursorAgentHost', scope).get('remoteInferenceRoute');
    if (route !== 'always') {
      return vscode.window.showWarningMessage(`Remote window: cursorAgentHost.remoteInferenceRoute = ${route ?? 'not available'}. For local BYOK set it to always in Remote Settings and reload this window. If unavailable, check your Cursor version.`);
    }
    return vscode.window.showInformationMessage('Remote window configured for local inference (always). Reload this window after changes. Private-inference policy can override this setting.');
  }));
  const mcp = vscode.cursor?.mcp;
  if (!mcp?.registerServer || !mcp.unregisterServer) {
    throw new Error('BYOK Workspace Symbols requires Cursor’s MCP registration API.');
  }
  const registered = new Map();
  function add(folder) {
    const uri = folder.uri.toString();
    if (registered.has(uri)) return;
    const slug = folder.name.replace(/[^a-zA-Z0-9_-]/g, '').slice(0, 8);
    const id = createHash('sha256').update(uri).digest('hex').slice(0, 10);
    const configuration = workspace.getConfiguration('byok.workspaceSymbols');
    const names = [`symbols-${slug}-${id}`, `semantic-${slug}-${id}`];
    mcp.registerServer({name: names[0], server: {
      command: configuration.get('executable', 'serena'),
      args: ['start-mcp-server', '--transport', 'stdio', '--agent-interface', 'tools', '--language-backend', 'LSP', '--project', folder.uri.fsPath,
        '--context', context.asAbsolutePath('symbols.yml'), '--enable-web-dashboard', 'false', '--open-web-dashboard', 'false'],
      env: {},
    }});
    mcp.registerServer({name: names[1], server: {
      command: process.execPath,
      args: [context.asAbsolutePath('semantic.cjs'), folder.uri.fsPath, configuration.get('semanticExecutable', 'semble')],
      env: {ELECTRON_RUN_AS_NODE: '1', SEMBLE_MCP_IDLE_TIMEOUT: '300', TOKENIZERS_PARALLELISM: 'false'},
    }});
    registered.set(uri, names);
  }
  function remove(uri) {
    const names = registered.get(uri);
    if (names === undefined) return;
    for (const name of names) mcp.unregisterServer(name);
    registered.delete(uri);
  }
  context.subscriptions.push({dispose() {for (const uri of registered.keys()) remove(uri);}});
  for (const folder of workspace.workspaceFolders ?? []) add(folder);
  context.subscriptions.push(workspace.onDidChangeWorkspaceFolders(({added, removed}) => {
    for (const folder of removed) remove(folder.uri.toString());
    for (const folder of added) add(folder);
  }));
}
module.exports = {activate};
