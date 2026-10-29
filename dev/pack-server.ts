// Builds the Linux server archive (ADR-0043): a static oc-server (musl), the pinned
// livekit-server, the systemd unit and install.sh — `ourcamp-server-<ver>-linux-<arch>.tar.gz`
// with its SHA-256 beside it, in target/pack/.
//
// Run on Linux, after `pnpm livekit:fetch`: `pnpm server:pack`. Needs `musl-tools`
// (musl-gcc: ring and SQLite are C) and `rustup target add <arch>-unknown-linux-musl`.
// The archive is for the machine's own architecture: livekit-server is the one fetched for it.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");

/** Node's architecture → the musl target and the name in the archive. */
const TARGETS: Record<string, { target: string; arch: string }> = {
  x64: { target: "x86_64-unknown-linux-musl", arch: "x64" },
  arm64: { target: "aarch64-unknown-linux-musl", arch: "arm64" },
};

/** What goes into the archive from packaging/linux, and whether it is a program. */
const PACKAGING: [file: string, executable: boolean][] = [
  ["install.sh", true],
  ["ourcamp", true],
  ["certbot-deploy-hook.sh", true],
  ["ourcamp-server.service", false],
  ["server.env", false],
  ["README.txt", false],
];

function version(): string {
  const manifest = readFileSync(join(root, "crates", "oc-server", "Cargo.toml"), "utf8");
  const found = /^version = "([^"]+)"/m.exec(manifest);
  if (found === null) throw new Error("no version in crates/oc-server/Cargo.toml");
  return found[1] as string;
}

function main(): void {
  if (process.platform !== "linux") throw new Error("the Linux archive is built on Linux (CI: the server-linux job)");
  const chosen = TARGETS[process.arch];
  if (chosen === undefined) throw new Error(`no Linux server build for ${process.arch}`);
  const livekit = join(root, "dev", "bin", "livekit-server");
  if (!existsSync(livekit)) throw new Error("no dev/bin/livekit-server: run `pnpm livekit:fetch` first");

  // cc-rs would look for `<arch>-linux-musl-gcc`, which Ubuntu does not have.
  const cc = `CC_${chosen.target.replaceAll("-", "_")}`;
  execFileSync("cargo", ["build", "--release", "--locked", "-p", "oc-server", "--target", chosen.target], {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, [cc]: process.env[cc] ?? "musl-gcc" },
  });

  const name = `ourcamp-server-${version()}-linux-${chosen.arch}`;
  const pack = join(root, "target", "pack");
  const stage = join(pack, name);
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });

  const put = (from: string, file: string, executable: boolean): void => {
    copyFileSync(from, join(stage, file));
    chmodSync(join(stage, file), executable ? 0o755 : 0o644);
  };
  put(join(root, "target", chosen.target, "release", "oc-server"), "oc-server", true);
  put(livekit, "livekit-server", true);
  for (const [file, executable] of PACKAGING) put(join(root, "packaging", "linux", file), file, executable);

  const archive = join(pack, `${name}.tar.gz`);
  // Owned by root in the archive, whoever built it: install.sh copies, not extracts in place.
  execFileSync("tar", ["--owner=0", "--group=0", "--numeric-owner", "-czf", archive, "-C", pack, name], {
    stdio: "inherit",
  });
  const sha256 = createHash("sha256").update(readFileSync(archive)).digest("hex");
  writeFileSync(`${archive}.sha256`, `${sha256}  ${name}.tar.gz\n`);
  console.log(`${archive}\nsha256 ${sha256}`);
}

try {
  main();
} catch (e: unknown) {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
}
