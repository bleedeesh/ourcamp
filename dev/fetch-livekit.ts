// Downloads the pinned livekit-server into dev/bin/ for `managed` mode (ADR-0022).
// Run: `pnpm livekit:fetch`. In M8 the server installer ships the binary instead.
//
// The SHA-256 of every archive is pinned here, not read from the release's own
// checksums.txt: a replaced archive would come with a matching checksums.txt.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Same version as dev/compose.yaml: upgrade both together.
const VERSION = "1.13.9";

/** `${platform}-${arch}` → release archive and its SHA-256. */
const ARCHIVES: Record<string, { file: string; sha256: string }> = {
  "win32-x64": {
    file: `livekit_${VERSION}_windows_amd64.zip`,
    sha256: "d5b5e44c197f613fdd6bdb2ae53e9d086d848156d43f614c688c6b595cd3f25e",
  },
  "win32-arm64": {
    file: `livekit_${VERSION}_windows_arm64.zip`,
    sha256: "bc34c226ea047299d0bc08c9f6071b8d52c13f57be08dd5085f794c9095334a6",
  },
  "linux-x64": {
    file: `livekit_${VERSION}_linux_amd64.tar.gz`,
    sha256: "0b7fa208b662d09cfdeae8c06cf4c481aead0556086558b48250501e2e2d6e20",
  },
  "linux-arm64": {
    file: `livekit_${VERSION}_linux_arm64.tar.gz`,
    sha256: "c5e4e5a6d6cf0a449651aa3178532ecc7a2bf2502538396ebe6e51ff13424056",
  },
};

const binDir = join(import.meta.dirname, "bin");
const exe = process.platform === "win32" ? "livekit-server.exe" : "livekit-server";
const target = join(binDir, exe);

function versionOf(path: string): string {
  return execFileSync(path, ["--version"], { encoding: "utf8" }).trim();
}

async function main(): Promise<void> {
  const key = `${process.platform}-${process.arch}`;
  const archive = ARCHIVES[key];
  if (archive === undefined) {
    if (process.platform === "darwin") {
      throw new Error("LiveKit publishes no macOS binary: `brew install livekit` (the server finds it on PATH)");
    }
    throw new Error(`no livekit-server ${VERSION} build for ${key}`);
  }
  if (existsSync(target) && versionOf(target).includes(VERSION)) {
    console.log(`livekit-server ${VERSION} is already in ${binDir}`);
    return;
  }

  const url = `https://github.com/livekit/livekit/releases/download/v${VERSION}/${archive.file}`;
  console.log(`downloading ${url}`);
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== archive.sha256) {
    throw new Error(`${archive.file}: SHA-256 ${actual}, expected ${archive.sha256} — not using it`);
  }

  // Unpacked in a scratch directory; only the verified binary is moved into dev/bin.
  const scratch = mkdtempSync(join(tmpdir(), "oc-livekit-"));
  try {
    const file = join(scratch, archive.file);
    writeFileSync(file, bytes);
    // Windows: its own bsdtar (10+), which also unpacks .zip — not a GNU tar from Git
    // that may come first on PATH and reads `C:` as a remote host.
    const tar =
      process.platform === "win32" ? join(process.env["SystemRoot"] ?? "C:\\Windows", "System32", "tar.exe") : "tar";
    execFileSync(tar, ["-xf", file, "-C", scratch], { stdio: "inherit" });
    mkdirSync(binDir, { recursive: true });
    rmSync(target, { force: true });
    renameSync(join(scratch, exe), target);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
  console.log(`${versionOf(target)} → ${target}`);
}

main().catch((e: unknown) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
