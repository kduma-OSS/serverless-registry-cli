// Unit tests of scripts/bump-version.mjs: node --test scripts/

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, it } from "node:test";

import {
  BumpError,
  compareVersions,
  describeChange,
  nextVersion,
  parseVersion,
  readCargoVersion,
  rollChangelog,
  run,
  updateCargoLock,
  updateCargoToml,
} from "./bump-version.mjs";

describe("nextVersion", () => {
  const cases = [
    ["1.2.0-beta.3", "prerelease", "1.2.0-beta.4"],
    ["1.2.0", "prerelease", "1.2.1-beta.1"],
    ["1.2.0-rc.1", "prerelease", "1.2.0-rc.2"],
    ["1.2.0-beta", "prerelease", "1.2.0-beta.1"],
    ["1.2.0-beta.9", "prerelease", "1.2.0-beta.10"],
    ["1.2.0", "prepatch", "1.2.1-beta.1"],
    ["1.2.0-beta.3", "prepatch", "1.2.1-beta.1"],
    ["1.2.3", "preminor", "1.3.0-beta.1"],
    ["1.2.3", "premajor", "2.0.0-beta.1"],
    ["1.2.3", "patch", "1.2.4"],
    ["1.2.0-beta.3", "patch", "1.2.0"],
    ["1.2.3", "minor", "1.3.0"],
    ["1.3.0-beta.1", "minor", "1.3.0"],
    ["1.2.1-beta.1", "minor", "1.3.0"],
    ["1.2.3", "major", "2.0.0"],
    ["2.0.0-beta.1", "major", "2.0.0"],
    ["2.1.0-beta.1", "major", "3.0.0"],
    ["1.2.0-beta.3", "release", "1.2.0"],
    ["1.2.0", "none", "1.2.0"],
    ["1.2.0-beta.3", "none", "1.2.0-beta.3"],
  ];
  for (const [current, bump, expected] of cases) {
    it(`${bump}: ${current} -> ${expected}`, () => {
      assert.equal(nextVersion(current, bump), expected);
    });
  }

  it("takes a greater custom version", () => {
    assert.equal(nextVersion("1.2.0", "custom", "1.2.1-rc.1"), "1.2.1-rc.1");
    assert.equal(nextVersion("1.2.0-beta.3", "custom", "1.2.0"), "1.2.0");
  });

  it("rejects invalid input", () => {
    assert.throws(() => nextVersion("1.2.0", "release"), /not a prerelease/);
    assert.throws(() => nextVersion("1.2.0", "custom", "1.2.0"), /not greater/);
    assert.throws(() => nextVersion("1.2.0", "custom", "1.1.9"), /not greater/);
    assert.throws(() => nextVersion("1.2.0", "custom", "v1.3.0"), /without the leading v/);
    assert.throws(() => nextVersion("1.2.0", "custom", "1.3"), /not a semver version/);
    assert.throws(() => nextVersion("1.2.0", "custom", "01.3.0"), /not a semver version/);
    assert.throws(() => nextVersion("1.2.0", "custom", "1.3.0+build.1"), /build metadata/);
    assert.throws(() => nextVersion("1.2.0", "custom"), /needs a version/);
    assert.throws(() => nextVersion("1.2.0", "patch", "1.3.0"), /only accepted with/);
    assert.throws(() => nextVersion("1.2.0", "bogus"), /unknown bump/);
    assert.throws(() => nextVersion("1.2", "patch"), BumpError);
  });
});

describe("compareVersions", () => {
  it("follows semver precedence", () => {
    const ordered = [
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0-beta",
      "1.0.0-beta.2",
      "1.0.0-beta.11",
      "1.0.0-rc.1",
      "1.0.0",
      "1.0.1",
      "1.1.0",
      "2.0.0",
    ];
    for (let i = 0; i + 1 < ordered.length; i++) {
      assert.ok(
        compareVersions(ordered[i], ordered[i + 1]) < 0,
        `${ordered[i]} < ${ordered[i + 1]}`,
      );
      assert.ok(compareVersions(ordered[i + 1], ordered[i]) > 0);
    }
    assert.equal(compareVersions("1.0.0", "1.0.0"), 0);
    assert.deepEqual(parseVersion("1.2.3-beta.1+b5"), {
      major: 1,
      minor: 2,
      patch: 3,
      prerelease: ["beta", "1"],
      build: "b5",
    });
  });
});

const HEADER = `# Changelog

All notable changes to this project will be documented in this file.
`;
const BASE = "https://github.com/acme/tool";

describe("rollChangelog", () => {
  it("moves the Unreleased entries under a new section and updates the links", () => {
    const text = `${HEADER}
## [Unreleased]

### Added

- New thing.

## [1.0.0] - 2026-01-01

### Added

- First.

[Unreleased]: ${BASE}/compare/v1.0.0...HEAD
[1.0.0]: ${BASE}/releases/tag/v1.0.0
`;
    const { text: out, notes } = rollChangelog(text, "1.1.0-beta.1", "2026-10-28");
    assert.equal(
      out,
      `${HEADER}
## [Unreleased]

## [1.1.0-beta.1] - 2026-10-28

### Added

- New thing.

## [1.0.0] - 2026-01-01

### Added

- First.

[Unreleased]: ${BASE}/compare/v1.1.0-beta.1...HEAD
[1.1.0-beta.1]: ${BASE}/compare/v1.0.0...v1.1.0-beta.1
[1.0.0]: ${BASE}/releases/tag/v1.0.0
`,
    );
    assert.equal(notes, "### Added\n\n- New thing.");
  });

  it("merges the Unreleased entries into an existing section and updates its date", () => {
    const text = `${HEADER}
## [Unreleased]

Intro text.

### Fixed

- A fix.

### Added

- Later thing.

### Security

- Patched.

## [0.1.0] - 2026-10-01

### Added

- Early thing.

### Changed

- A change.

### Fixed

- Old fix.

[Unreleased]: ${BASE}/compare/v0.1.0...HEAD
[0.1.0]: ${BASE}/releases/tag/v0.1.0
`;
    const { text: out, notes } = rollChangelog(text, "0.1.0", "2026-10-28", { allowEmpty: true });
    assert.equal(
      out,
      `${HEADER}
## [Unreleased]

## [0.1.0] - 2026-10-28

Intro text.

### Added

- Early thing.
- Later thing.

### Changed

- A change.

### Fixed

- Old fix.
- A fix.

### Security

- Patched.

[Unreleased]: ${BASE}/compare/v0.1.0...HEAD
[0.1.0]: ${BASE}/releases/tag/v0.1.0
`,
    );
    assert.match(notes, /^Intro text\.\n\n### Added\n/);
    assert.match(notes, /- Patched\.$/);
  });

  it("places a missing group in the usual order", () => {
    const text = `## [Unreleased]

### Changed

- Changed.

## [2.0.0] - 2026-01-01

### Added

- Added.

### Fixed

- Fixed.
`;
    const { text: out } = rollChangelog(text, "2.0.0", "2026-02-02");
    assert.match(out, /### Added\n\n- Added\.\n\n### Changed\n\n- Changed\.\n\n### Fixed\n/);
  });

  it("updates only the date of an existing section when Unreleased is empty", () => {
    const text = `## [Unreleased]

## [0.1.0] - 2026-10-01

### Added

- Thing.

[Unreleased]: ${BASE}/compare/v0.1.0...HEAD
[0.1.0]: ${BASE}/releases/tag/v0.1.0
`;
    const { text: out, notes } = rollChangelog(text, "0.1.0", "2026-10-28");
    assert.equal(out, text.replace("2026-10-01", "2026-10-28"));
    assert.equal(notes, "### Added\n\n- Thing.");
  });

  it("folds the prerelease sections into the stable release", () => {
    const text = [
      "# Changelog",
      "",
      "## [Unreleased]",
      "",
      "### Fixed",
      "",
      "- A late fix.",
      "",
      "## [1.1.0-beta.2] - 2026-10-27",
      "",
      "### Added",
      "",
      "- Second feature.",
      "",
      "## [1.1.0-beta.1] - 2026-10-26",
      "",
      "### Added",
      "",
      "- First feature.",
      "",
      "## [1.0.0] - 2026-10-01",
      "",
      "### Added",
      "",
      "- Everything.",
      "",
      "[Unreleased]: https://example.test/compare/v1.1.0-beta.2...HEAD",
      "[1.1.0-beta.2]: https://example.test/compare/v1.1.0-beta.1...v1.1.0-beta.2",
      "[1.1.0-beta.1]: https://example.test/compare/v1.0.0...v1.1.0-beta.1",
      "[1.0.0]: https://example.test/releases/tag/v1.0.0",
      "",
    ].join("\n");
    const { text: out, notes } = rollChangelog(text, "1.1.0", "2026-10-28");
    assert.doesNotMatch(out, /beta/);
    assert.match(out, /## \[Unreleased\]\n\n## \[1\.1\.0\] - 2026-10-28\n/);
    assert.match(notes, /### Added\n\n- First feature\.\n- Second feature\.\n\n### Fixed\n\n- A late fix\./);
    assert.match(out, /\[Unreleased\]: https:\/\/example\.test\/compare\/v1\.1\.0\.\.\.HEAD\n\[1\.1\.0\]: https:\/\/example\.test\/compare\/v1\.0\.0\.\.\.v1\.1\.0\n\[1\.0\.0\]/);
  });

  it("refuses to release nothing", () => {
    const text = `## [Unreleased]

## [1.0.0] - 2026-01-01

- First.
`;
    assert.throws(() => rollChangelog(text, "1.0.1", "2026-10-28"), /nothing to release/);
    const { text: out, notes } = rollChangelog(text, "1.0.1", "2026-10-28", { allowEmpty: true });
    assert.match(out, /## \[Unreleased\]\n\n## \[1\.0\.1\] - 2026-10-28\n\n## \[1\.0\.0\]/);
    assert.equal(notes, "");
  });

  it("needs an Unreleased section", () => {
    assert.throws(
      () => rollChangelog("# Changelog\n", "1.0.0", "2026-10-28"),
      /no ## \[Unreleased\]/,
    );
  });

  it("adds a release link when there was no previous release", () => {
    const text = `## [Unreleased]

- First.

[Unreleased]: ${BASE}/compare/v0.0.0...HEAD
`;
    const { text: out } = rollChangelog(text, "0.1.0", "2026-10-28");
    assert.match(
      out,
      /\[Unreleased\]: .*\/compare\/v0\.1\.0\.\.\.HEAD\n\[0\.1\.0\]: .*\/compare\/v0\.0\.0\.\.\.v0\.1\.0\n$/,
    );
  });
});

describe("Cargo files", () => {
  const cargoToml = `[workspace]
members = ["crates/core"]

[workspace.package]
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
sreg-core = { path = "crates/core", version = "0.1.0" }
serde = { version = "1.0.229", features = ["derive"] }

[profile.release]
lto = "thin"
`;

  it("reads and sets the workspace version and the internal dependencies", () => {
    assert.equal(readCargoVersion(cargoToml), "0.1.0");
    const out = updateCargoToml(cargoToml, "0.2.0-beta.1");
    assert.equal(readCargoVersion(out), "0.2.0-beta.1");
    assert.match(out, /sreg-core = \{ path = "crates\/core", version = "0\.2\.0-beta\.1" \}/);
    assert.match(out, /serde = \{ version = "1\.0\.229"/);
    assert.throws(() => readCargoVersion('[package]\nversion = "1.0.0"\n'), /no version/);
  });

  it("sets the version of the workspace packages in Cargo.lock only", () => {
    const lock = `version = 4

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "sreg-core"
version = "0.1.0"
dependencies = [
 "serde",
]
`;
    const out = updateCargoLock(lock, "0.2.0");
    assert.equal(
      out,
      lock.replace(
        'name = "sreg-core"\nversion = "0.1.0"',
        'name = "sreg-core"\nversion = "0.2.0"',
      ),
    );
  });
});

describe("describeChange", () => {
  it("lists the changed lines", () => {
    assert.equal(
      describeChange("f", "a\nb\nc\nd\n", "a\nB\nc\nd\ne\n"),
      "--- f\n@@ line 2\n-b\n+B\n@@ line 5\n+e",
    );
  });
});

describe("run", () => {
  function fixture() {
    const root = mkdtempSync(path.join(tmpdir(), "bump-version-"));
    writeFileSync(
      path.join(root, "Cargo.toml"),
      '[workspace.package]\nversion = "1.2.0-beta.3"\n\n[workspace.dependencies]\nx = { path = "x", version = "1.2.0-beta.3" }\n',
    );
    writeFileSync(
      path.join(root, "Cargo.lock"),
      '[[package]]\nname = "x"\nversion = "1.2.0-beta.3"\n',
    );
    writeFileSync(
      path.join(root, "CHANGELOG.md"),
      `## [Unreleased]\n\n### Fixed\n\n- Fix.\n\n[Unreleased]: ${BASE}/compare/v1.2.0-beta.3...HEAD\n`,
    );
    return root;
  }

  it("prints the next version without touching the files", () => {
    const root = fixture();
    try {
      const before = readFileSync(path.join(root, "Cargo.toml"), "utf8");
      assert.equal(run(["prerelease", "--root", root]), "1.2.0-beta.4");
      const logs = [];
      assert.equal(
        run(["release", "--dry-run", "--root", root], { log: (t) => logs.push(t) }),
        "1.2.0",
      );
      assert.match(logs.join("\n"), /\+## \[1\.2\.0\] - \d{4}-\d{2}-\d{2}/);
      assert.equal(readFileSync(path.join(root, "Cargo.toml"), "utf8"), before);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("applies the version and writes the release notes", () => {
    const root = fixture();
    try {
      const notes = path.join(root, "notes.md");
      const version = run([
        "release",
        "--apply",
        "--date",
        "2026-10-28",
        "--notes-file",
        notes,
        "--root",
        root,
      ]);
      assert.equal(version, "1.2.0");
      const read = (file) => readFileSync(path.join(root, file), "utf8");
      assert.equal(readCargoVersion(read("Cargo.toml")), "1.2.0");
      assert.match(read("Cargo.toml"), /x = \{ path = "x", version = "1\.2\.0" \}/);
      assert.match(read("Cargo.lock"), /version = "1\.2\.0"\n$/);
      assert.match(
        read("CHANGELOG.md"),
        /## \[Unreleased\]\n\n## \[1\.2\.0\] - 2026-10-28\n\n### Fixed\n\n- Fix\.\n/,
      );
      assert.equal(read("notes.md"), "### Fixed\n\n- Fix.\n");
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("rejects bad options", () => {
    assert.throws(() => run([]), /expected <bump>/);
    assert.throws(() => run(["patch", "--apply", "--dry-run"]), /either --apply or --dry-run/);
    assert.throws(() => run(["patch", "--date", "28.10.2026"]), /not YYYY-MM-DD/);
    assert.throws(() => run(["patch", "--frobnicate"]), /unknown option/);
  });
});
