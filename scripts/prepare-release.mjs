// Set the crate version for a release.
//
//   node scripts/prepare-release.mjs --set 0.4.0    explicit target version
//   node scripts/prepare-release.mjs --bump minor   patch|minor|major from current
//
// Rewrites the `version = "..."` line in the [package] table of Cargo.toml
// (dependency `version = ...` keys never start a line, so a line-anchored
// match only hits the package version). Cargo.lock is synced afterwards by
// `cargo check` in the release workflow. Refuses to move backwards or to the
// current version. Prints "<crate>: <old> -> <new>".

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const manifestPath = path.join(root, "Cargo.toml");
const text = readFileSync(manifestPath, "utf8");

const args = process.argv.slice(2);
const argValue = (flag) => {
  const i = args.indexOf(flag);
  if (i === -1) return null;
  const v = args[i + 1];
  if (v == null || v.startsWith("--")) {
    console.error(`error: ${flag} expects a value`);
    process.exit(1);
  }
  return v;
};
const set = argValue("--set");
const bump = argValue("--bump");

if (args.includes("-h") || args.includes("--help") || (set == null && bump == null) || (set != null && bump != null)) {
  console.log(
    "usage: node scripts/prepare-release.mjs --set <0.4.0> | --bump <patch|minor|major>",
  );
  if (!args.includes("-h") && !args.includes("--help")) process.exit(1);
  process.exit(0);
}

const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;
const parse = (v) => {
  const m = SEMVER.exec(v);
  return m ? { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] ?? null } : null;
};
const coreLess = (a, b) => {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i];
  return false;
};

const nameMatch = /^\s*name\s*=\s*"([^"]+)"/m.exec(text);
if (!nameMatch) {
  console.error("error: could not find package name in Cargo.toml");
  process.exit(1);
}
const crateName = nameMatch[1];
const versionMatch = /^version\s*=\s*"([^"]*)"/m.exec(text);
if (!versionMatch) {
  console.error("error: could not find package version in Cargo.toml");
  process.exit(1);
}

const current = parse(versionMatch[1]);
if (!current) {
  console.error(`error: current version "${versionMatch[1]}" is not semver (x.y.z)`);
  process.exit(1);
}

let next;
if (set != null) {
  next = parse(set);
  if (!next) {
    console.error(`error: --set "${set}" is not semver (x.y.z, optional -prerelease)`);
    process.exit(1);
  }
} else if (!["patch", "minor", "major"].includes(bump)) {
  console.error(`error: --bump "${bump}" must be patch, minor, or major`);
  process.exit(1);
} else {
  const [a, b, c] = current.core;
  next = {
    core:
      bump === "major"
        ? [a + 1, 0, 0]
        : bump === "minor"
          ? [a, b + 1, 0]
          : [a, b, c + 1],
    pre: null,
  };
}

const version = `${next.core.join(".")}${next.pre ? `-${next.pre}` : ""}`;
if (version === versionMatch[1]) {
  console.error(`error: already at version ${version}`);
  process.exit(1);
}
if (coreLess(next.core, current.core)) {
  console.error(`error: refusing to move backwards (${versionMatch[1]} -> ${version})`);
  process.exit(1);
}

writeFileSync(
  manifestPath,
  text.replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`),
);
console.log(`${crateName}: ${versionMatch[1]} -> ${version}`);
