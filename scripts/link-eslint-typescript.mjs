#!/usr/bin/env node
// typescript-eslint (the ESLint tooling) doesn't yet support the TS 7 native compiler this
// project pins for `tsc`/Vite: real API differences inside typescript-estree, not just a
// version-string guard (https://github.com/typescript-eslint/typescript-eslint/issues/10940).
//
// pnpm's packageExtensions/overrides could not be made to give the @typescript-eslint/*
// packages (and the third-party tools they pull in, e.g. ts-api-utils) an isolated
// `typescript`, because they also declare it as a peerDependency and pnpm prefers the hoisted
// peer over an injected one of the same name. So this script does directly, right after
// install, what those mechanisms are meant for: it finds every package under node_modules/.pnpm
// whose own `node_modules/typescript` symlink resolves to the project's real (TS 7) install,
// and repoints it at the pinned `typescript5-for-eslint` install (an aliased `typescript@5.7.3`,
// added as a devDependency) instead. Matched by following each symlink rather than by the
// pnpm store's folder-naming convention, which isn't always a literal "_typescript@<version>"
// suffix (pnpm hashes the suffix when a package resolves several peers together).
// `tsc --noEmit` and the Vite build never go through this and keep using the real TypeScript.
import { existsSync, readdirSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const pnpmDir = join(root, "node_modules", ".pnpm");
const isolated = join(root, "node_modules", "typescript5-for-eslint");
const realTypescript = join(root, "node_modules", "typescript");

if (!existsSync(pnpmDir) || !existsSync(isolated) || !existsSync(realTypescript)) {
  console.log("link-eslint-typescript: nothing to do (missing node_modules/.pnpm, typescript5-for-eslint or typescript)");
  process.exit(0);
}

const realTarget = realpathSync(realTypescript);
const isolatedTarget = realpathSync(isolated);

let relinked = 0;
for (const entry of readdirSync(pnpmDir)) {
  // Leave the real `typescript` package's own store folder untouched.
  if (entry.startsWith("typescript@")) continue;
  const link = join(pnpmDir, entry, "node_modules", "typescript");
  if (!existsSync(link)) continue;
  let current;
  try {
    current = realpathSync(link);
  } catch {
    continue; // broken symlink; leave it for pnpm to sort out
  }
  if (current !== realTarget || current === isolatedTarget) continue;
  rmSync(link, { force: true });
  symlinkSync(isolated, link, "junction");
  relinked++;
  console.log(`link-eslint-typescript: ${entry} -> typescript@5.7.3 (was pointing at the real typescript@7)`);
}
console.log(`link-eslint-typescript: relinked ${relinked} package(s)`);
