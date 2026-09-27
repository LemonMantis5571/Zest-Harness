// Feature map: navigation docs for agents and contributors, kept honest by CI.
//
// Every entry in docs/features/*.md starts with frontmatter naming the files
// that implement the feature and the tests that cover it. This script checks
// that map against the tree so it cannot silently rot:
//
//   - every listed path or glob must match a file that exists;
//   - every source file under crates/ and scripts/ must belong to a feature
//     (generated bindings are the only exemption);
//   - the index table in docs/features/README.md must match the entries.
//
// It is also the lookup tool:
//
//   node ./scripts/feature-map.mjs check [--staged]   validate (default command)
//   node ./scripts/feature-map.mjs where <path>...    which features own a file
//   node ./scripts/feature-map.mjs show <slug>        a feature's files, tests, checks
//   node ./scripts/feature-map.mjs list               every feature, one line each
//   node ./scripts/feature-map.mjs index              rewrite the README index table
//
// `where`, `show`, and `list` accept --json.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const FEATURES_DIR = "docs/features";
const INDEX_FILE = `${FEATURES_DIR}/README.md`;
const INDEX_START = "<!-- feature-index:start -->";
const INDEX_END = "<!-- feature-index:end -->";

// Files that must be claimed by at least one feature.
const COVERED_EXTENSIONS = new Set([".rs", ".ts", ".tsx", ".mjs", ".js", ".ps1", ".sh"]);
const COVERED_ROOTS = ["crates/", "scripts/"];

// Exact prefixes that never need an owner, each with the reason.
export const COVERAGE_EXEMPT = [
  // ts-rs output; the owning feature is the Rust type it was generated from.
  "crates/desktop/ui/src/lib/generated/",
];

const LIST_KEYS = new Set(["paths", "tests", "verify"]);
const SCALAR_KEYS = new Set(["title", "summary"]);

function runGit(args) {
  // Capture stderr: a failed `git show :<path>` is an expected, handled case.
  return execFileSync("git", args, {
    cwd: repoRoot,
    maxBuffer: 16 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  }).toString("utf8");
}

function nulList(text) {
  return text.split("\0").filter((entry) => entry.length > 0);
}

export function normalizeRepoPath(filePath) {
  let normalized = filePath.replaceAll("\\", "/");
  const root = repoRoot.replaceAll("\\", "/");
  if (normalized.toLowerCase().startsWith(`${root.toLowerCase()}/`)) {
    normalized = normalized.slice(root.length + 1);
  }
  return normalized.replace(/^\.\//, "");
}

/** Parse the small YAML subset the feature docs use: scalars and string lists. */
export function parseFrontmatter(text, source = "feature doc") {
  const lines = text.replace(/^﻿/, "").split(/\r?\n/);
  if (lines[0] !== "---") throw new Error(`${source}: must start with a --- frontmatter block`);
  const end = lines.indexOf("---", 1);
  if (end < 0) throw new Error(`${source}: frontmatter block is not closed with ---`);

  const data = {};
  let listKey = null;
  for (let index = 1; index < end; index += 1) {
    const line = lines[index];
    const lineNo = index + 1;
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;

    const item = /^\s+-\s+(.+?)\s*$/.exec(line);
    if (item) {
      if (!listKey) throw new Error(`${source}:${lineNo}: list item outside a list key`);
      data[listKey].push(unquote(item[1]));
      continue;
    }

    const pair = /^([a-z]+):\s*(.*?)\s*$/.exec(line);
    if (!pair) throw new Error(`${source}:${lineNo}: expected "key: value" or "  - item"`);
    const [, key, value] = pair;
    if (key in data) throw new Error(`${source}:${lineNo}: duplicate key "${key}"`);
    if (LIST_KEYS.has(key)) {
      if (value !== "") throw new Error(`${source}:${lineNo}: "${key}" must be a list of "  - item" lines`);
      data[key] = [];
      listKey = key;
    } else if (SCALAR_KEYS.has(key)) {
      if (value === "") throw new Error(`${source}:${lineNo}: "${key}" needs a value`);
      data[key] = unquote(value);
      listKey = null;
    } else {
      throw new Error(`${source}:${lineNo}: unknown key "${key}"`);
    }
  }

  for (const key of SCALAR_KEYS) {
    if (!data[key]) throw new Error(`${source}: missing "${key}"`);
  }
  if (!data.paths?.length) throw new Error(`${source}: "paths" must list at least one file`);
  data.tests ??= [];
  data.verify ??= [];
  return { data, body: lines.slice(end + 1).join("\n") };
}

function unquote(value) {
  const quoted = /^(["'])(.*)\1$/.exec(value);
  return quoted ? quoted[2] : value;
}

/** Exact file, directory prefix ending in "/", or glob with * and **. */
export function compilePattern(pattern) {
  const normalized = normalizeRepoPath(pattern);
  if (normalized.endsWith("/")) return (file) => file.startsWith(normalized);
  if (!normalized.includes("*")) return (file) => file === normalized;
  const source = normalized
    .split(/(\*\*\/?|\*)/)
    .map((part) => {
      if (part === "**/") return "(?:.*/)?";
      if (part === "**") return ".*";
      if (part === "*") return "[^/]*";
      return part.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
    })
    .join("");
  const regex = new RegExp(`^${source}$`);
  return (file) => regex.test(file);
}

export function loadFeatures(readDir = defaultReadDir, readFile = defaultReadFile) {
  return readDir(FEATURES_DIR)
    .filter((name) => name.endsWith(".md") && name !== "README.md")
    .sort()
    .map((name) => {
      const doc = `${FEATURES_DIR}/${name}`;
      const { data } = parseFrontmatter(readFile(doc), doc);
      return { slug: name.slice(0, -3), doc, ...data };
    });
}

function defaultReadDir(dir) {
  const absolute = path.join(repoRoot, dir);
  return existsSync(absolute) ? readdirSync(absolute) : [];
}

function defaultReadFile(file) {
  return readFileSync(path.join(repoRoot, file), "utf8");
}

function safeRead(readFile, file) {
  try {
    return readFile(file);
  } catch {
    return "";
  }
}

export function isCovered(file) {
  if (!COVERED_ROOTS.some((root) => file.startsWith(root))) return false;
  if (COVERAGE_EXEMPT.some((prefix) => file.startsWith(prefix))) return false;
  return COVERED_EXTENSIONS.has(path.posix.extname(file));
}

function ownersOf(file, features) {
  const owners = [];
  for (const feature of features) {
    if (feature.paths.some((pattern) => compilePattern(pattern)(file))) {
      owners.push({ slug: feature.slug, role: "code" });
    } else if (feature.tests.some((pattern) => compilePattern(pattern)(file))) {
      owners.push({ slug: feature.slug, role: "test" });
    }
  }
  return owners;
}

/** Stale patterns, unowned source files, and duplicate slugs. */
export function validateFeatureMap(features, files) {
  const errors = [];
  for (const feature of features) {
    for (const key of ["paths", "tests"]) {
      for (const pattern of feature[key]) {
        const matches = compilePattern(pattern);
        if (!files.some(matches)) {
          errors.push(`${feature.doc}: ${key} entry "${pattern}" matches no file (renamed or deleted?)`);
        }
      }
    }
  }

  const unmapped = files.filter((file) => isCovered(file) && ownersOf(file, features).length === 0);
  for (const file of unmapped) {
    errors.push(`${file}: not listed by any feature (add it to the owning docs/features/*.md)`);
  }
  return errors;
}

function plural(count, noun) {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

export function renderIndex(features) {
  const rows = features.map((feature) => {
    // Rust tests live inline, so a feature can be well covered with no test
    // files; its verify commands are what run them.
    const checks = [];
    if (feature.tests.length > 0) checks.push(plural(feature.tests.length, "test file"));
    if (feature.verify.length > 0) checks.push(plural(feature.verify.length, "command"));
    const coverage = checks.length > 0 ? checks.join(", ") : "**none**";
    return `| [${feature.title}](${feature.slug}.md) | ${feature.summary.replaceAll("|", "\\|")} | ${coverage} |`;
  });
  return [INDEX_START, "| Feature | What it does | Checks |", "| --- | --- | --- |", ...rows, INDEX_END].join(
    "\n",
  );
}

function spliceIndex(readme, features) {
  const start = readme.indexOf(INDEX_START);
  const end = readme.indexOf(INDEX_END);
  if (start < 0 || end < start) {
    throw new Error(`${INDEX_FILE}: missing ${INDEX_START} ... ${INDEX_END} markers`);
  }
  return readme.slice(0, start) + renderIndex(features) + readme.slice(end + INDEX_END.length);
}

function listFiles({ staged }) {
  if (staged) return nulList(runGit(["ls-files", "-z", "--cached"]));
  // Working tree: tracked files that still exist plus new files not yet added.
  return nulList(runGit(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])).filter((file) =>
    existsSync(path.join(repoRoot, file)),
  );
}

/** Features whose code is in `changed` while their doc is not. */
export function findUntouchedDocs(features, changed) {
  const changedSet = new Set(changed);
  const notes = [];
  for (const feature of features) {
    if (changedSet.has(feature.doc)) continue;
    const touched = changed.filter((file) => feature.paths.some((pattern) => compilePattern(pattern)(file)));
    if (touched.length > 0) notes.push({ feature, touched });
  }
  return notes;
}

/** Readers over the index, so a pre-commit check sees exactly what is committed. */
function stagedReaders(files) {
  const prefix = `${FEATURES_DIR}/`;
  return {
    readDir: () => files.filter((file) => file.startsWith(prefix)).map((file) => file.slice(prefix.length)),
    readFile: (file) => runGit(["show", `:${file}`]),
  };
}

function check(args) {
  const staged = args.includes("--staged");
  const files = listFiles({ staged });
  const readers = staged ? stagedReaders(files) : { readDir: defaultReadDir, readFile: defaultReadFile };
  const features = loadFeatures(readers.readDir, readers.readFile);
  const errors = validateFeatureMap(features, files);

  const readme = safeRead(readers.readFile, INDEX_FILE);
  try {
    if (spliceIndex(readme, features) !== readme) {
      errors.push(`${INDEX_FILE}: index table is out of date (run: npm run features -- index)`);
    }
  } catch (error) {
    errors.push(error.message);
  }

  if (errors.length > 0) {
    const shown = 30;
    console.error("Feature map check failed:");
    for (const error of errors.slice(0, shown)) console.error(`- ${error}`);
    if (errors.length > shown) console.error(`- ... and ${errors.length - shown} more`);
    process.exitCode = 1;
    return;
  }

  if (staged) {
    const changed = nulList(runGit(["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMRD"]));
    const notes = findUntouchedDocs(features, changed);
    if (notes.length > 0) {
      console.log("Feature map: this commit changes feature code without touching its doc.");
      for (const { feature, touched } of notes) {
        console.log(`  ${feature.doc} (${touched.length} file(s))`);
      }
      console.log("  Update the doc if user-visible behavior or entry points changed.");
    }
  }
  console.log(`Feature map check passed (${features.length} features).`);
}

function where(args, json) {
  const features = loadFeatures();
  const files = listFiles({ staged: false });
  const targets = args.filter((arg) => !arg.startsWith("--")).map(normalizeRepoPath);
  if (targets.length === 0) throw new Error("usage: feature-map.mjs where <path>...");

  const results = targets.map((target) => {
    const prefix = target.endsWith("/") ? target : `${target}/`;
    const matched = files.includes(target) ? [target] : files.filter((file) => file.startsWith(prefix));
    const counts = new Map();
    for (const file of matched) {
      for (const owner of ownersOf(file, features)) {
        const key = `${owner.slug}\0${owner.role}`;
        counts.set(key, (counts.get(key) ?? 0) + 1);
      }
    }
    const owners = [...counts].map(([key, count]) => {
      const [slug, role] = key.split("\0");
      return { slug, role, files: count, doc: `${FEATURES_DIR}/${slug}.md` };
    });
    return { path: target, files: matched.length, owners };
  });

  if (json) {
    console.log(JSON.stringify(results, null, 2));
    return;
  }
  for (const result of results) {
    if (result.files === 0) {
      console.log(`${result.path}: no such file or directory`);
    } else if (result.owners.length === 0) {
      console.log(`${result.path}: no feature (exempt or unmapped)`);
    } else {
      console.log(`${result.path}:`);
      for (const owner of result.owners) {
        const scope = result.files > 1 ? ` (${owner.files} file(s))` : "";
        console.log(`  ${owner.doc} [${owner.role}]${scope}`);
      }
    }
  }
}

function show(args, json) {
  const slug = args.find((arg) => !arg.startsWith("--"));
  const features = loadFeatures();
  const feature = features.find((candidate) => candidate.slug === slug);
  if (!feature) {
    throw new Error(`unknown feature "${slug ?? ""}". Known: ${features.map((f) => f.slug).join(", ")}`);
  }
  const files = listFiles({ staged: false });
  const expand = (patterns) => files.filter((file) => patterns.some((pattern) => compilePattern(pattern)(file)));
  const result = {
    slug: feature.slug,
    title: feature.title,
    summary: feature.summary,
    doc: feature.doc,
    files: expand(feature.paths),
    tests: expand(feature.tests),
    verify: feature.verify,
  };

  if (json) {
    console.log(JSON.stringify(result, null, 2));
    return;
  }
  console.log(`${result.title} (${result.doc})\n${result.summary}\n`);
  console.log(`Files (${result.files.length}):`);
  for (const file of result.files) console.log(`  ${file}`);
  console.log(`\nTests (${result.tests.length}):`);
  for (const file of result.tests) console.log(`  ${file}`);
  if (result.verify.length > 0) {
    console.log("\nVerify:");
    for (const command of result.verify) console.log(`  ${command}`);
  }
}

function list(json) {
  const features = loadFeatures();
  if (json) {
    console.log(JSON.stringify(features.map(({ slug, title, summary, doc }) => ({ slug, title, summary, doc })), null, 2));
    return;
  }
  const width = Math.max(...features.map((feature) => feature.slug.length));
  for (const feature of features) console.log(`${feature.slug.padEnd(width)}  ${feature.summary}`);
}

function writeIndex() {
  const features = loadFeatures();
  const readme = defaultReadFile(INDEX_FILE);
  const next = spliceIndex(readme, features);
  if (next === readme) {
    console.log(`${INDEX_FILE} is up to date.`);
    return;
  }
  writeFileSync(path.join(repoRoot, INDEX_FILE), next);
  console.log(`Rewrote the index in ${INDEX_FILE} (${features.length} features).`);
}

function main() {
  const [command = "check", ...args] = process.argv.slice(2);
  const json = args.includes("--json");
  switch (command) {
    case "check":
      return check(args);
    case "where":
      return where(args, json);
    case "show":
      return show(args, json);
    case "list":
      return list(json);
    case "index":
      return writeIndex();
    default:
      throw new Error(`unknown command "${command}". Use check, where, show, list, or index.`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error.stderr?.toString("utf8") || error.message);
    process.exitCode = 1;
  }
}
