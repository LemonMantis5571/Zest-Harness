import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outputPrefix = "outputs/";

export const OUTPUT_SIZE_LIMIT_BYTES = 1024 * 1024;

const mediaExtensions = new Set([
  ".aac",
  ".aif",
  ".aiff",
  ".avif",
  ".avi",
  ".bmp",
  ".flac",
  ".gif",
  ".heic",
  ".heif",
  ".jpeg",
  ".jpg",
  ".m4a",
  ".m4v",
  ".mkv",
  ".mov",
  ".mp3",
  ".mp4",
  ".mpeg",
  ".mpg",
  ".ogg",
  ".png",
  ".tif",
  ".tiff",
  ".wav",
  ".webm",
  ".webp",
  ".wmv",
]);

// Keep this list empty by default. Add exact repository-relative paths only
// when a reviewed output artifact is intentionally part of the source tree.
const allowedOutputArtifacts = new Set([]);

function runGit(args, input) {
  return execFileSync("git", args, {
    cwd: repoRoot,
    input,
    maxBuffer: 16 * 1024 * 1024,
  });
}

function nulRecords(buffer) {
  return buffer
    .toString("utf8")
    .split("\0")
    .filter((record) => record.length > 0);
}

function normalizeRepoPath(filePath) {
  return filePath.replaceAll("\\", "/");
}

function parseTreeEntries(commit = "HEAD", git = runGit) {
  const records = nulRecords(git(["ls-tree", "-r", "-l", "-z", commit, "--", outputPrefix]));

  return records.flatMap((record) => {
    const separator = record.indexOf("\t");
    if (separator < 0) return [];

    const [mode, type, objectId, sizeText] = record.slice(0, separator).split(" ");
    if (type !== "blob") return [];

    const size = Number(sizeText);
    if (!Number.isSafeInteger(size) || size < 0) {
      throw new Error(`Could not read tracked size for ${record.slice(separator + 1)}`);
    }

    return [
      {
        path: normalizeRepoPath(record.slice(separator + 1)),
        size,
        mode,
        objectId,
      },
    ];
  });
}

export function parseCommitRangeEntries(base, head, git = runGit) {
  const validObjectId = /^[\da-f]{40,64}$/i;
  if (!validObjectId.test(base) || !validObjectId.test(head)) {
    throw new Error("Commit range requires full base and head object IDs.");
  }

  const commits = git(["rev-list", "--reverse", `${base}..${head}`])
    .toString("utf8")
    .trim()
    .split(/\r?\n/)
    .filter(Boolean);
  return commits.flatMap((commit) => {
    const parentRecord = git(["rev-list", "--parents", "-n", "1", commit])
      .toString("utf8")
      .trim();
    const [, ...parents] = parentRecord.split(" ");
    const diffArgs = [
      "diff-tree",
      "--no-commit-id",
      "--name-only",
      "--diff-filter=ACMR",
      "--no-renames",
      "-r",
      "-z",
    ];
    if (parents.length === 0) {
      diffArgs.push("--root", commit);
    } else {
      diffArgs.push(parents[0], commit);
    }
    diffArgs.push("--", outputPrefix);

    const changedPaths = new Set(
      nulRecords(git(diffArgs)).map(normalizeRepoPath),
    );
    if (changedPaths.size === 0) return [];

    return parseTreeEntries(commit, git).filter(({ path: filePath }) =>
      changedPaths.has(filePath),
    );
  });
}

function parseStagedEntries() {
  const changedPaths = new Set(
    nulRecords(
      runGit([
        "diff",
        "--cached",
        "--name-only",
        "--diff-filter=ACMR",
        "-z",
        "--",
        "outputs/",
      ]),
    ).map(normalizeRepoPath),
  );

  if (changedPaths.size === 0) return [];

  const records = nulRecords(runGit(["ls-files", "--stage", "-z", "--", "outputs/"]));
  const staged = records.flatMap((record) => {
    const separator = record.indexOf("\t");
    if (separator < 0) return [];

    const [mode, objectId, stage] = record.slice(0, separator).split(" ");
    const filePath = normalizeRepoPath(record.slice(separator + 1));
    if (stage !== "0" || !changedPaths.has(filePath)) return [];

    return [{ path: filePath, mode, objectId }];
  });

  if (staged.length === 0) return [];

  const objectIds = [...new Set(staged.map(({ objectId }) => objectId))];
  const sizeRecords = runGit(
    ["cat-file", "--batch-check=%(objectname) %(objectsize)"],
    Buffer.from(`${objectIds.join("\n")}\n`, "utf8"),
  )
    .toString("utf8")
    .trim()
    .split("\n");
  const sizes = new Map(
    sizeRecords.map((record) => {
      const [objectId, sizeText] = record.split(" ");
      return [objectId, Number(sizeText)];
    }),
  );

  return staged.map(({ path: filePath, mode, objectId }) => ({
    path: filePath,
    mode,
    objectId,
    size: sizes.get(objectId),
  }));
}

export function findOutputArtifactViolations(entries, allowlist = allowedOutputArtifacts) {
  const violations = [];

  for (const entry of entries) {
    const filePath = normalizeRepoPath(entry.path);
    if (!filePath.startsWith(outputPrefix) || allowlist.has(filePath)) continue;

    const extension = path.posix.extname(filePath).toLowerCase();
    const reasons = [];
    if (mediaExtensions.has(extension)) {
      reasons.push(`${extension} media files are not allowed under outputs/`);
    }
    if (entry.size > OUTPUT_SIZE_LIMIT_BYTES) {
      reasons.push(`size exceeds the 1 MiB limit (${(entry.size / 1024 / 1024).toFixed(2)} MiB)`);
    }

    if (reasons.length > 0) violations.push({ path: filePath, reasons });
  }

  return violations;
}

function main() {
  const mode = process.argv[2] ?? "--tracked";
  let entries;

  if (mode === "--staged") {
    entries = parseStagedEntries();
  } else if (mode === "--tracked") {
    entries = parseTreeEntries();
  } else if (mode === "--range" && process.argv.length === 5) {
    entries = parseCommitRangeEntries(process.argv[3], process.argv[4]);
  } else {
    throw new Error(
      `Unknown mode or arguments. Use --tracked, --staged, or --range <base-sha> <head-sha>.`,
    );
  }

  const violations = findOutputArtifactViolations(entries);
  if (violations.length > 0) {
    console.error("Output artifact policy failed:");
    for (const violation of violations) {
      console.error(`- ${violation.path}: ${violation.reasons.join("; ")}`);
    }
    console.error(
      "Move intentional media out of outputs/, reduce generated output size, or add a reviewed exact-path allowlist entry.",
    );
    process.exitCode = 1;
    return;
  }

  console.log(`Output artifact policy passed (${entries.length} output file(s) checked).`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error.stderr?.toString("utf8") || error.message);
    process.exitCode = 1;
  }
}
