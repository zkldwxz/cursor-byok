# BYOK Workspace Symbols

Each trusted Cursor workspace root registers two independent stdio MCP servers. The workspace host owns the source, indexes and language servers.

| Server | Use it for | Install on the host that holds the files |
|---|---|---|
| `semantic-…` | Find code by behavior or name and get one-based source lines | `uv tool install 'semble[mcp]==0.7.3'` |
| `symbols-…` | List symbols and find definitions, references and implementations | `uv tool install -p 3.13 serena-agent==2.1.0` |

1. Build the VSIX in this directory with `npx --package @vscode/vsce vsce package --no-dependencies`.
2. Use **Extensions: Install from VSIX** in the target Cursor window. In a Remote SSH window, install the extension and both tools on the SSH host. If the tools are outside the host's PATH, set `byok.workspaceSymbols.semanticExecutable` and `byok.workspaceSymbols.executable` to the respective absolute paths in the host settings, then reload.
3. Enable `semantic-…` and `symbols-…` for the desired root in Cursor's MCP settings. Ask for code that performs a known behavior, or for the definition and references of a known function. Serena reports zero-based positions. Grep, Glob and Read handle exact text, file paths and source reads.

Semble embeds locally and may download its model on first use. Set `SEMBLE_MODEL_NAME` to a model directory on the workspace host for offline use. Semble releases its in-memory index after 300 seconds of inactivity and keeps the disk index for reuse. Serena may install the language server for the project; some languages also require a local toolchain. Both tools use stdio without gateway credentials or a reverse tunnel. `npm run check` runs the extension tests.
