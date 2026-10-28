#!/usr/bin/env node
// Computes the next release version and, with --apply, writes it into the version files and
// rolls CHANGELOG.md. Used by .github/workflows/bump-and-release.yml and runnable locally:
//
//   node scripts/bump-version.mjs <bump> [<version>] [--apply | --dry-run]
//                                 [--date YYYY-MM-DD] [--notes-file <path>] [--root <dir>]
//   node scripts/bump-version.mjs --current
//
// It prints the next version (or, with --current, the current one) on stdout. --dry-run
// prints the changes it would make on stderr, --apply makes them, and --notes-file writes the
// release notes (the version's CHANGELOG.md section). See docs/releasing.md for the bumps.
// No dependencies: only Node's standard library.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

export const BUMPS = [
  "prerelease",
  "prepatch",
  "preminor",
  "premajor",
  "patch",
  "minor",
  "major",
  "release",
  "none",
  "custom",
];

/** The prerelease label that new prereleases start with (`-beta.1`). */
export const PREID = "beta";

export class BumpError extends Error {}

const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$/;

/** Parses a strict semver version; `null` when it is not one. */
export function parseVersion(text) {
  const match = SEMVER.exec(text);
  if (match === null) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] === undefined ? [] : match[4].split("."),
    build: match[5] ?? "",
  };
}

export function formatVersion({ major, minor, patch, prerelease }) {
  const base = `${major}.${minor}.${patch}`;
  return prerelease.length === 0 ? base : `${base}-${prerelease.join(".")}`;
}

function parseOrThrow(text, what) {
  const version = parseVersion(text);
  if (version === null) {
    const hint = /^v\d/.test(text) ? " (write it without the leading v)" : "";
    throw new BumpError(`${what} ${JSON.stringify(text)} is not a semver version${hint}`);
  }
  return version;
}

function isNumeric(identifier) {
  return /^\d+$/.test(identifier);
}

function compareIdentifiers(a, b) {
  const aNum = isNumeric(a);
  const bNum = isNumeric(b);
  if (aNum && bNum) return Math.sign(Number(a) - Number(b));
  if (aNum) return -1;
  if (bNum) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Semver precedence: negative, zero or positive like a sort comparator. */
export function compareVersions(left, right) {
  const a = typeof left === "string" ? parseOrThrow(left, "version") : left;
  const b = typeof right === "string" ? parseOrThrow(right, "version") : right;
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] !== b[key]) return Math.sign(a[key] - b[key]);
  }
  // A prerelease comes before its release.
  if (a.prerelease.length === 0 && b.prerelease.length === 0) return 0;
  if (a.prerelease.length === 0) return 1;
  if (b.prerelease.length === 0) return -1;
  const length = Math.max(a.prerelease.length, b.prerelease.length);
  for (let i = 0; i < length; i++) {
    if (a.prerelease[i] === undefined) return -1;
    if (b.prerelease[i] === undefined) return 1;
    const result = compareIdentifiers(a.prerelease[i], b.prerelease[i]);
    if (result !== 0) return result;
  }
  return 0;
}

/**
 * The version after `bump` applied to `current`, as a string. `custom` is the requested
 * version for the `custom` bump and must be empty otherwise.
 */
export function nextVersion(current, bump, custom = "") {
  const v = parseOrThrow(current, "the current version");
  if (!BUMPS.includes(bump)) {
    throw new BumpError(
      `unknown bump ${JSON.stringify(bump)}; expected one of ${BUMPS.join(", ")}`,
    );
  }
  if (bump !== "custom" && custom !== "") {
    throw new BumpError(`a version (${custom}) is only accepted with the custom bump`);
  }
  const stable = v.prerelease.length === 0;
  const { major, minor, patch } = v;
  const make = (ma, mi, pa, prerelease = []) =>
    formatVersion({ major: ma, minor: mi, patch: pa, prerelease });
  switch (bump) {
    case "prerelease": {
      if (stable) return make(major, minor, patch + 1, [PREID, "1"]);
      const prerelease = [...v.prerelease];
      const last = prerelease.length - 1;
      if (isNumeric(prerelease[last])) {
        prerelease[last] = String(Number(prerelease[last]) + 1);
      } else {
        prerelease.push("1");
      }
      return make(major, minor, patch, prerelease);
    }
    case "prepatch":
      return make(major, minor, patch + 1, [PREID, "1"]);
    case "preminor":
      return make(major, minor + 1, 0, [PREID, "1"]);
    case "premajor":
      return make(major + 1, 0, 0, [PREID, "1"]);
    case "patch":
      return stable ? make(major, minor, patch + 1) : make(major, minor, patch);
    case "minor":
      return !stable && patch === 0 ? make(major, minor, 0) : make(major, minor + 1, 0);
    case "major":
      return !stable && minor === 0 && patch === 0 ? make(major, 0, 0) : make(major + 1, 0, 0);
    case "release":
      if (stable) {
        throw new BumpError(`${current} is not a prerelease; there is no suffix to drop`);
      }
      return make(major, minor, patch);
    case "none":
      return formatVersion(v);
    case "custom": {
      if (custom === "") throw new BumpError("the custom bump needs a version");
      const requested = parseOrThrow(custom, "the version");
      if (requested.build !== "") {
        throw new BumpError(`the version ${custom} must not carry build metadata`);
      }
      if (compareVersions(requested, v) <= 0) {
        throw new BumpError(`the version ${custom} is not greater than the current ${current}`);
      }
      return formatVersion(requested);
    }
  }
  throw new BumpError(`unhandled bump ${bump}`);
}

// ---------------------------------------------------------------------------------------------
// CHANGELOG.md (Keep a Changelog)

/** The usual order of the change groups, used to place a group that a section lacks. */
const GROUP_ORDER = ["Added", "Changed", "Deprecated", "Removed", "Fixed", "Security"];

const HEADING = /^## \[([^\]]+)\]/;
const LINK = /^\[[^\]]+\]:\s*\S/;

function trimBlank(lines) {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start++;
  while (end > start && lines[end - 1].trim() === "") end--;
  return lines.slice(start, end);
}

/** Index of the first line of the link definitions at the end of the file. */
function linksStart(lines) {
  let index = lines.length;
  while (index > 0 && lines[index - 1].trim() === "") index--;
  let start = index;
  while (start > 0 && LINK.test(lines[start - 1])) start--;
  return start === index ? lines.length : start;
}

function findSection(lines, name) {
  return lines.findIndex((line) => {
    const match = HEADING.exec(line);
    return match !== null && match[1].toLowerCase() === name.toLowerCase();
  });
}

function sectionEnd(lines, start) {
  const links = linksStart(lines);
  for (let i = start + 1; i < lines.length; i++) {
    if (lines[i].startsWith("## ") || i >= links) return i;
  }
  return lines.length;
}

/** Splits a section body into its free text and its `### Group` subsections. */
function splitGroups(lines) {
  const preamble = [];
  const groups = [];
  for (const line of lines) {
    if (line.startsWith("### ")) {
      groups.push({ heading: line, name: line.slice(4).trim(), lines: [] });
    } else if (groups.length === 0) {
      preamble.push(line);
    } else {
      groups[groups.length - 1].lines.push(line);
    }
  }
  return { preamble: trimBlank(preamble), groups };
}

function joinGroups({ preamble, groups }) {
  const out = [];
  if (preamble.length > 0) out.push(...preamble, "");
  for (const group of groups) {
    const body = trimBlank(group.lines);
    out.push(group.heading, "");
    if (body.length > 0) out.push(...body, "");
  }
  return trimBlank(out);
}

/** Merges the entries of `extra` (the Unreleased body) into `base` (an existing section). */
export function mergeSections(baseLines, extraLines) {
  const base = splitGroups(baseLines);
  const extra = splitGroups(extraLines);
  const preamble =
    extra.preamble.length === 0
      ? base.preamble
      : base.preamble.length === 0
        ? extra.preamble
        : [...base.preamble, "", ...extra.preamble];
  const groups = base.groups.map((group) => ({ ...group, lines: trimBlank(group.lines) }));
  for (const group of extra.groups) {
    const existing = groups.find((candidate) => candidate.name === group.name);
    if (existing !== undefined) {
      existing.lines = [...existing.lines, ...trimBlank(group.lines)];
      continue;
    }
    const rank = GROUP_ORDER.indexOf(group.name);
    const before =
      rank === -1
        ? -1
        : groups.findIndex((candidate) => GROUP_ORDER.indexOf(candidate.name) > rank);
    const entry = { ...group, lines: trimBlank(group.lines) };
    if (before === -1) groups.push(entry);
    else groups.splice(before, 0, entry);
  }
  return joinGroups({ preamble, groups });
}

/**
 * Moves the `[Unreleased]` entries under `## [<version>] - <date>` (merging them into that
 * section when it already exists, and updating its date), keeps an empty `[Unreleased]` and
 * updates the compare links. Returns the new text and the release notes (the section body).
 * Without entries and without an existing section there is nothing to release, which is an
 * error unless `allowEmpty` is set.
 */
export function rollChangelog(text, version, date, { allowEmpty = false } = {}) {
  let lines = text.replace(/\r\n/g, "\n").split("\n");
  const unreleased = findSection(lines, "Unreleased");
  if (unreleased === -1) throw new BumpError("CHANGELOG.md has no ## [Unreleased] section");
  let unreleasedEnd = sectionEnd(lines, unreleased);
  let entries = trimBlank(lines.slice(unreleased + 1, unreleasedEnd));
  // A stable release absorbs the sections of its own prereleases: their entries are merged
  // into the new section (oldest prerelease first, then the Unreleased entries), their
  // sections and links disappear, and the compare link starts where the first prerelease did.
  let previousStable = null;
  if (!version.includes("-")) {
    const folded = foldPrereleases(lines, version);
    if (folded.sections.length > 0) {
      lines = folded.lines;
      unreleasedEnd = sectionEnd(lines, unreleased);
      let merged = [];
      for (const body of folded.sections) merged = mergeSections(merged, body);
      entries = mergeSections(merged, entries);
      previousStable = folded.previous;
    }
  }
  const existing = findSection(lines, version);
  if (existing !== -1 && existing < unreleased) {
    throw new BumpError(`the ## [${version}] section must come after ## [Unreleased]`);
  }
  if (entries.length === 0 && existing === -1 && !allowEmpty) {
    throw new BumpError(
      `nothing to release: [Unreleased] in CHANGELOG.md is empty and there is no ## [${version}] section`,
    );
  }

  const heading = `## [${version}] - ${date}`;
  let out;
  if (existing === -1) {
    const body = entries.length > 0 ? [...entries, ""] : [];
    out = [
      ...lines.slice(0, unreleased + 1),
      "",
      heading,
      "",
      ...body,
      ...lines.slice(unreleasedEnd),
    ];
  } else {
    const existingEnd = sectionEnd(lines, existing);
    const merged = mergeSections(lines.slice(existing + 1, existingEnd), entries);
    out = [
      ...lines.slice(0, unreleased + 1),
      "",
      ...lines.slice(unreleasedEnd, existing),
      heading,
      "",
      ...(merged.length > 0 ? [...merged, ""] : []),
      ...lines.slice(existingEnd),
    ];
  }
  out = updateLinks(out, version, previousStable);

  const start = findSection(out, version);
  const notes = trimBlank(out.slice(start + 1, sectionEnd(out, start))).join("\n");
  return { text: out.join("\n"), notes };
}

/**
 * Removes the sections of the prereleases of `version` (`## [<version>-<pre>] - <date>`) and
 * their link lines. Returns the remaining lines, the bodies of the removed sections (oldest
 * first) and the tag the oldest of them was compared against, for the new compare link.
 */
function foldPrereleases(lines, version) {
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const heading = new RegExp(`^## \\[${escaped}-[0-9A-Za-z.-]+\\]`);
  const link = new RegExp(`^\\[${escaped}-[0-9A-Za-z.-]+\\]:\\s*\\S+/compare/(\\S+)\\.\\.\\.\\S+\\s*$`);
  const sections = [];
  let previous = null;
  let out = [...lines];
  for (;;) {
    const start = out.findIndex((line) => heading.test(line));
    if (start === -1) break;
    const end = sectionEnd(out, start);
    sections.unshift(trimBlank(out.slice(start + 1, end)));
    out = [...out.slice(0, start), ...out.slice(end)];
  }
  const kept = [];
  for (const line of out) {
    const match = link.exec(line);
    if (match === null) kept.push(line);
    else previous = match[1];
  }
  return { lines: kept, sections, previous };
}

function updateLinks(lines, version, previousStable = null) {
  const tag = `v${version}`;
  const index = lines.findIndex((line) => /^\[Unreleased\]:\s*\S/i.test(line));
  if (index === -1) return lines;
  const match = /^\[Unreleased\]:\s*(\S+)\/compare\/(\S+)\.\.\.HEAD\s*$/i.exec(lines[index]);
  if (match === null) return lines;
  const base = match[1];
  const previous = previousStable ?? match[2];
  if (previous === tag) return lines;
  const out = [...lines];
  out[index] = `[Unreleased]: ${base}/compare/${tag}...HEAD`;
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const hasLink = out.some((line) => new RegExp(`^\\[${escaped}\\]:`).test(line));
  if (!hasLink) out.splice(index + 1, 0, `[${version}]: ${base}/compare/${previous}...${tag}`);
  return out;
}

// ---------------------------------------------------------------------------------------------
// Version files of this repository

/** The `version` of `[workspace.package]` in the root Cargo.toml. */
export function readCargoVersion(cargoToml) {
  let table = "";
  for (const line of cargoToml.split("\n")) {
    const header = /^\s*\[([^\]]+)\]\s*$/.exec(line);
    if (header !== null) {
      table = header[1].trim();
      continue;
    }
    const version = /^\s*version\s*=\s*"([^"]*)"/.exec(line);
    if (table === "workspace.package" && version !== null) return version[1];
  }
  throw new BumpError("Cargo.toml has no version in [workspace.package]");
}

/**
 * Sets the version of `[workspace.package]` and of the internal (`path`) crates in
 * `[workspace.dependencies]`.
 */
export function updateCargoToml(cargoToml, version) {
  let table = "";
  let found = false;
  const lines = cargoToml.split("\n").map((line) => {
    const header = /^\s*\[([^\]]+)\]\s*$/.exec(line);
    if (header !== null) {
      table = header[1].trim();
      return line;
    }
    if (table === "workspace.package" && /^\s*version\s*=/.test(line)) {
      found = true;
      return line.replace(/"[^"]*"/, `"${version}"`);
    }
    if (table === "workspace.dependencies" && /\bpath\s*=/.test(line)) {
      return line.replace(/\bversion\s*=\s*"[^"]*"/, `version = "${version}"`);
    }
    return line;
  });
  if (!found) throw new BumpError("Cargo.toml has no version in [workspace.package]");
  return lines.join("\n");
}

/** Sets the version of the workspace's own packages (those without a `source`) in Cargo.lock. */
export function updateCargoLock(cargoLock, version) {
  return cargoLock
    .split(/(?=^\[\[package\]\]$)/m)
    .map((block) => {
      if (!block.startsWith("[[package]]") || /^source\s*=/m.test(block)) return block;
      return block.replace(/^version = "[^"]*"$/m, `version = "${version}"`);
    })
    .join("");
}

/** Reads the current version of this repository. */
export function readCurrentVersion(root) {
  return readCargoVersion(readFileSync(path.join(root, "Cargo.toml"), "utf8"));
}

/** The new contents of every version file, as `{ file: [before, after] }`. */
export function versionFileChanges(root, version) {
  const read = (file) => readFileSync(path.join(root, file), "utf8");
  const cargoToml = read("Cargo.toml");
  const cargoLock = read("Cargo.lock");
  return {
    "Cargo.toml": [cargoToml, updateCargoToml(cargoToml, version)],
    "Cargo.lock": [cargoLock, updateCargoLock(cargoLock, version)],
  };
}

// ---------------------------------------------------------------------------------------------
// Command line

function parseArgs(argv) {
  const options = {
    apply: false,
    dryRun: false,
    current: false,
    date: "",
    notesFile: "",
    root: "",
  };
  const positional = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new BumpError(`${arg} needs a value`);
      return argv[++i];
    };
    if (arg === "--apply") options.apply = true;
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--current") options.current = true;
    else if (arg === "--date") options.date = value();
    else if (arg === "--notes-file") options.notesFile = value();
    else if (arg === "--root") options.root = value();
    else if (arg === "--help" || arg === "-h") options.help = true;
    else if (arg.startsWith("--")) throw new BumpError(`unknown option ${arg}`);
    else positional.push(arg);
  }
  if (options.apply && options.dryRun) throw new BumpError("use either --apply or --dry-run");
  if (options.date !== "" && !/^\d{4}-\d{2}-\d{2}$/.test(options.date)) {
    throw new BumpError(`--date ${options.date} is not YYYY-MM-DD`);
  }
  if (!options.help && !options.current && (positional.length < 1 || positional.length > 2)) {
    throw new BumpError("expected <bump> [<version>]");
  }
  return { options, bump: positional[0], custom: positional[1] ?? "" };
}

function tagExists(root, tag) {
  try {
    return (
      execFileSync("git", ["-C", root, "tag", "--list", tag], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      }).trim() !== ""
    );
  } catch {
    return false; // not a git checkout: nothing to compare with
  }
}

/** The changed lines of a file (a line diff without context), for --dry-run. */
export function describeChange(file, before, after) {
  const a = before.split("\n");
  const b = after.split("\n");
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  // Longest common subsequence of the differing middle part.
  const x = a.slice(start, endA);
  const y = b.slice(start, endB);
  const width = y.length + 1;
  const lcs = new Uint32Array((x.length + 1) * width);
  for (let i = x.length - 1; i >= 0; i--) {
    for (let j = y.length - 1; j >= 0; j--) {
      lcs[i * width + j] =
        x[i] === y[j]
          ? lcs[(i + 1) * width + j + 1] + 1
          : Math.max(lcs[(i + 1) * width + j], lcs[i * width + j + 1]);
    }
  }
  const out = [`--- ${file}`];
  let i = 0;
  let j = 0;
  let inHunk = false;
  while (i < x.length || j < y.length) {
    if (i < x.length && j < y.length && x[i] === y[j]) {
      i++;
      j++;
      inHunk = false;
      continue;
    }
    if (!inHunk) out.push(`@@ line ${start + j + 1}`);
    inHunk = true;
    if (j >= y.length || (i < x.length && lcs[(i + 1) * width + j] >= lcs[i * width + j + 1])) {
      out.push(`-${x[i++]}`);
    } else {
      out.push(`+${y[j++]}`);
    }
  }
  return out.join("\n");
}

/** Runs the command line; returns the next version. */
export function run(argv, { log = (text) => process.stderr.write(`${text}\n`) } = {}) {
  const { options, bump, custom } = parseArgs(argv);
  if (options.help) {
    log(
      "usage: node scripts/bump-version.mjs <bump> [<version>] [--apply | --dry-run]\n" +
        "         [--date YYYY-MM-DD] [--notes-file <path>] [--root <dir>]\n" +
        "       node scripts/bump-version.mjs --current [--root <dir>]\n" +
        `bumps: ${BUMPS.join(", ")}`,
    );
    return "";
  }
  const root =
    options.root !== ""
      ? path.resolve(options.root)
      : path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const current = readCurrentVersion(root);
  if (options.current) return current;
  const version = nextVersion(current, bump, custom);
  const tag = `v${version}`;
  if (tagExists(root, tag)) throw new BumpError(`the tag ${tag} already exists`);
  if (!options.apply && !options.dryRun && options.notesFile === "") return version;

  const date = options.date !== "" ? options.date : new Date().toISOString().slice(0, 10);
  const changes = versionFileChanges(root, version);
  const changelog = readFileSync(path.join(root, "CHANGELOG.md"), "utf8");
  const rolled = rollChangelog(changelog, version, date, { allowEmpty: bump === "none" });
  changes["CHANGELOG.md"] = [changelog, rolled.text];

  if (options.dryRun) log(`${current} -> ${version} (${bump}), tag ${tag}, date ${date}`);
  for (const [file, [before, after]] of Object.entries(changes)) {
    if (before === after) continue;
    if (options.dryRun) log(describeChange(file, before, after));
    if (options.apply) writeFileSync(path.join(root, file), after);
  }
  if (options.notesFile !== "") writeFileSync(options.notesFile, `${rolled.notes}\n`);
  return version;
}

if (
  process.argv[1] !== undefined &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  try {
    const version = run(process.argv.slice(2));
    if (version !== "") process.stdout.write(`${version}\n`);
  } catch (error) {
    if (!(error instanceof BumpError)) throw error;
    process.stderr.write(`error: ${error.message}\n`);
    process.exit(1);
  }
}
