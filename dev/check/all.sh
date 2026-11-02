#!/bin/sh
# Everything before a PR, on Windows in Git Bash: types, TS and Rust tests, tests with a real
# LiveKit (the app's five times over: they catch races), lint. Takes about half an hour.
# Needs `pnpm livekit:fetch` once. Logs go to target/check.
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT" || exit 2
LOGS=target/check
mkdir -p "$LOGS"
failed=0

# Sums the `test result` lines of cargo's output into one line.
total() {
  awk -v what="$1" '/test result/ {p+=$4; f+=$6} END {print what ":", p+0, "passed,", f+0, "failed"; exit f > 0 || p == 0}'
}

pnpm typecheck > "$LOGS/typecheck.log" 2>&1 || { echo "typecheck: FAILED"; failed=1; }
pnpm test > "$LOGS/test.log" 2>&1 || { echo "pnpm test: FAILED"; failed=1; }

./dev/bin/livekit-server.exe --config dev/check/livekit.yaml > "$LOGS/livekit.log" 2>&1 &
LIVEKIT=$!
sleep 2
for i in 1 2 3 4 5; do
  # One thread: these tests share one room (ADR-0053).
  cargo test -p ourcamp-desktop --lib -- --ignored --nocapture --test-threads=1 > "$LOGS/app-$i.log" 2>&1
  grep -q "test result: ok" "$LOGS/app-$i.log" || { echo "app with LiveKit, run $i: FAILED, see $LOGS/app-$i.log"; failed=1; }
done
cargo test -p oc-server -p oc-camp-client -p oc-link-preview -- --include-ignored > "$LOGS/server.log" 2>&1
total "server and client, with LiveKit" < "$LOGS/server.log" || failed=1
kill "$LIVEKIT"

cargo test --workspace > "$LOGS/workspace.log" 2>&1
total "workspace" < "$LOGS/workspace.log" || failed=1
pnpm lint:rust > "$LOGS/lint.log" 2>&1 || { echo "lint: FAILED, see $LOGS/lint.log"; failed=1; }

if [ "$failed" = 0 ]; then echo "all green"; else echo "SOMETHING FAILED"; fi
exit "$failed"
