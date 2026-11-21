//! Binds upstream Semble MCP to the workspace that owns its index.
const { spawn } = require('node:child_process');
const { isAbsolute } = require('node:path');
const { createInterface } = require('node:readline');

const methods = new Set(['search', 'find_related']);
const instructions = 'Use search for a natural-language code location in this workspace. Results contain one-based source lines. Use the language server for definitions and references and Grep for exact literals.';
const descriptions = {
  search: 'Locate an implementation in this workspace by behavior or name. Returns file paths and 1-based lines.',
  find_related: 'Find similar code in this workspace from a file path and one-based line number.',
};

function bindRequest(request, root) {
  if (request.method === 'tools/call' && methods.has(request.params?.name)) {
    request.params.arguments = { ...request.params.arguments, repo: root };
  }
  return request;
}

function bindResponse(response) {
  const result = response.result;
  if (result?.serverInfo) result.instructions = instructions;
  if (Array.isArray(result?.tools)) {
    for (const tool of result.tools.filter(tool => methods.has(tool.name))) {
      tool.description = descriptions[tool.name];
      delete tool.inputSchema.properties.repo;
      tool.inputSchema.required = tool.inputSchema.required.filter(name => name !== 'repo');
    }
  }
  // Semble 0.7.3 reports an indexing error as a text result. Give Cursor an
  // actual MCP error so a failed index cannot be mistaken for zero matches.
  if (result?.content?.some(part => part.type === 'text' && part.text.startsWith('Failed to index '))) {
    result.isError = true;
  }
  return response;
}

function serve(root, executable) {
  if (!root || !isAbsolute(root) || !executable) throw new Error('Semble MCP requires an absolute workspace root and a Semble executable');
  const child = spawn(executable, [], { cwd: root, stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true });
  const requests = createInterface({ input: process.stdin, crlfDelay: Infinity });
  const responses = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const write = (stream, value) => stream.write(JSON.stringify(value) + '\n');
  requests.on('line', line => { if (line.trim()) write(child.stdin, bindRequest(JSON.parse(line), root)); });
  responses.on('line', line => { if (line.trim()) write(process.stdout, bindResponse(JSON.parse(line))); });
  requests.once('close', () => child.stdin.end());
  const end = code => {
    requests.close();
    process.stdin.pause();
    process.exitCode = code;
  };
  child.on('error', error => { console.error(`Start Semble MCP: ${error.message}`); end(1); });
  child.stdin.on('error', error => { console.error(`Write to Semble MCP: ${error.message}`); child.kill(); end(1); });
  child.once('close', (code, signal) => end(code ?? (signal ? 1 : 0)));
  process.once('SIGTERM', () => child.kill('SIGTERM'));
  process.once('SIGINT', () => child.kill('SIGINT'));
}

if (require.main === module) serve(process.argv[2], process.argv[3]);
module.exports = { bindRequest, bindResponse };
