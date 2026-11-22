# BYOK Workspace Symbols

Each trusted Cursor workspace root registers two independent stdio MCP servers. The workspace host owns the source, indexes and language servers.

| Server | Use it for | Install on the host that holds the files |
|---|---|---|
| `semantic-…` | Find code by behavior or name and get one-based source lines | `cargo install --path apps/semantic-mcp` |
| `symbols-…` | List symbols and find definitions, references and implementations | Nothing — it queries the language extensions already loaded in the window |

1. Build the VSIX in this directory with `npx --package @vscode/vsce vsce package --no-dependencies`.
2. Use **Extensions: Install from VSIX** in the target Cursor window. In a Remote SSH window, install the extension and `byok-semantic` on the SSH host. If the tool is outside the host's PATH, set `byok.workspaceSymbols.semanticExecutable` to its absolute path in the host settings, then reload.
3. Enable `semantic-…` and `symbols-…` for the desired root in Cursor's MCP settings. Ask for code that performs a known behavior, or for the definition and references of a known function. All tools report one-based positions. Grep, Glob and Read handle exact text, file paths and source reads.

`byok-semantic` embeds locally and downloads its model on first use. The model and per-workspace indexes live under `~/.cursor-byok-v3/cache/semble`; set `SEMBLE_CACHE_LOCATION` to move that cache elsewhere on the workspace host. The disk index is kept and refreshed incrementally on each call. `symbols-…` runs no process of its own: each workspace root gets a local socket answered by the extension host, and the registered stdio command is a relay that pipes the agent's MCP traffic to it. Definitions, references, implementations and document symbols come from the same providers as F12 and Shift+F12 in the editor, so languages work as soon as their extension is installed. Both tools use stdio without gateway credentials or a reverse tunnel. `npm run check` runs the extension tests.
