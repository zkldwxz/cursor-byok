import { appendFile, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

/** Reads docs/releases/v<version>.md; a release without written changes fails. */
export async function releaseNotes({ root, version }) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`invalid semantic version: ${version}`);
  }
  const path = `docs/releases/v${version}.md`;
  let written;
  try {
    written = (await readFile(join(root, path), "utf8")).trim();
  } catch (error) {
    throw new Error(`${path} is required to release ${version}: ${error.message}`);
  }
  if (!/^- \S/m.test(written)) {
    throw new Error(`${path} must list the main changes of ${version} as bullets`);
  }
  const beta = version.includes("-") ? "Beta release. " : "";
  return `${written}\n\n---\n\n${beta}Download the installer for your platform from the assets below.\n`;
}

async function main() {
  const [version, output] = process.argv.slice(2);
  if (!version) throw new Error("usage: release-notes.mjs <version> [output file]");
  const notes = await releaseNotes({ root: resolve("."), version });
  if (output) await writeFile(output, notes);
  else process.stdout.write(notes);
  if (process.env.GITHUB_OUTPUT) {
    const delimiter = `notes_${crypto.randomUUID()}`;
    await appendFile(process.env.GITHUB_OUTPUT, `notes<<${delimiter}\n${notes}${delimiter}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await main();
}
