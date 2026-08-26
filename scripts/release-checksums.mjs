import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { mkdir, readdir, writeFile } from "node:fs/promises";
import { basename, dirname, extname, relative, resolve } from "node:path";

const root = resolve(option("--root") ?? "target/release/bundle");
const output = resolve(option("--out") ?? "SHA256SUMS.txt");
const allowedExtensions = new Set([".appimage", ".deb", ".dmg", ".exe", ".msi", ".rpm"]);

const artifacts = (await filesUnder(root))
  .filter((file) => allowedExtensions.has(extname(file).toLowerCase()))
  .sort((left, right) => left.localeCompare(right));

if (artifacts.length === 0) {
  throw new Error(`No release installers found under ${root}`);
}

const names = new Set();
const lines = [];
for (const file of artifacts) {
  const name = basename(file);
  if (!names.add(name)) {
    throw new Error(`Duplicate release artifact name: ${name}`);
  }
  const hash = await sha256(file);
  lines.push(`${hash}  ${name}`);
}

await mkdir(dirname(output), { recursive: true });
await writeFile(output, `${lines.join("\n")}\n`, "utf8");

console.log(lines.join("\n"));
console.log(`\nWrote ${relative(process.cwd(), output) || basename(output)}`);

function option(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} needs a value`);
  }
  return value;
}

async function filesUnder(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await filesUnder(path)));
    } else if (entry.isFile()) {
      files.push(path);
    }
  }
  return files;
}

function sha256(path) {
  return new Promise((resolveHash, reject) => {
    const hash = createHash("sha256");
    const stream = createReadStream(path);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", () => resolveHash(hash.digest("hex")));
  });
}
