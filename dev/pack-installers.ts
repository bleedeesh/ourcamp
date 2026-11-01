// Builds the Windows installers (ADR-0074): the client and the server window with its
// livekit-server, each with its SHA-256 beside it, in target/pack/.
//
// Run on Windows, after `pnpm livekit:fetch`: `pnpm installers`. The first run also downloads
// NSIS and takes a while: everything is built for release.

import { createHash } from "node:crypto";
import { execSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const root = join(import.meta.dirname, "..");
/** What Tauri wants in the name of a program it carries beside the app. */
const TARGET = "x86_64-pc-windows-msvc";

function build(project: string, extra: string): void {
  execSync(`pnpm --filter ${project} tauri build --bundles nsis ${extra}`, { cwd: root, stdio: "inherit" });
}

function main(): void {
  if (process.platform !== "win32") throw new Error("only the Windows installers are built so far (ADR-0074)");
  const livekit = join(root, "dev", "bin", "livekit-server.exe");
  if (!existsSync(livekit)) throw new Error("no dev/bin/livekit-server.exe: run `pnpm livekit:fetch` first");

  const binaries = join(root, "apps", "server", "src-tauri", "binaries");
  mkdirSync(binaries, { recursive: true });
  copyFileSync(livekit, join(binaries, `livekit-server-${TARGET}.exe`));

  build("@ourcamp/desktop", "");
  // A separate config: with it in the main one, every `cargo build` would need the binary.
  build("@ourcamp/server", `--config "${join(root, "apps", "server", "src-tauri", "tauri.installer.json")}"`);

  const bundles = join(root, "target", "release", "bundle", "nsis");
  const pack = join(root, "target", "pack");
  mkdirSync(pack, { recursive: true });
  for (const file of readdirSync(bundles).filter((name) => name.endsWith("-setup.exe"))) {
    copyFileSync(join(bundles, file), join(pack, file));
    const sha256 = createHash("sha256").update(readFileSync(join(bundles, file))).digest("hex");
    writeFileSync(join(pack, `${file}.sha256`), `${sha256}  ${file}\n`);
    console.log(`${join(pack, file)}\n  ${sha256}`);
  }
}

main();
