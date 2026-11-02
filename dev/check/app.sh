#!/bin/sh
# A scenario in the real app (Windows, Git Bash): LiveKit and a camp; the scenario starts clients
# itself and drives them over WebView2's debug port (ADR-0047).
#
#   sh dev/check/app.sh dev/check/open-link.mjs
#
# The client is built with its pages inside (ADR-0074), so no Vite and no fight for port 5173.
# Clients use profiles named `check-*`, removed before and after. Needs `pnpm livekit:fetch`.
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT" || exit 2
LOGS=target/check
PROFILES="$APPDATA/app.ourcamp.desktop/profiles"
mkdir -p "$LOGS"
rm -rf "$LOGS/camp" "$PROFILES"/check-*

pnpm --filter @ourcamp/desktop tauri build --debug --no-bundle > "$LOGS/build.log" 2>&1 || {
  echo "the client did not build, see $LOGS/build.log"
  exit 2
}
cargo build -p oc-server || exit 2

./dev/bin/livekit-server.exe --config dev/check/livekit.yaml > "$LOGS/livekit.log" 2>&1 &
PIDS=$!
sleep 2
./target/debug/oc-server --data "$LOGS/camp" --bind 127.0.0.1:7799 --host 127.0.0.1 \
  --livekit-url ws://127.0.0.1:7880 --livekit-key devkey --livekit-secret secret > "$LOGS/camp.log" 2>&1 &
PIDS="$! $PIDS"
sleep 5

LINK=$(grep -aoE "ourcamp://[^ ]+" "$LOGS/camp.log" | head -1 | tr -d '\r')
if [ -z "$LINK" ]; then
  echo "the camp printed no link, see $LOGS/camp.log"
  code=2
else
  OC_LINK="$LINK" OC_EXE="$(pwd -W)/target/debug/ourcamp-desktop.exe" node "$@"
  code=$?
fi

# Only what this script started, by PID: never a real OurCamp or LiveKit by name.
for pid in $PIDS; do kill "$pid" 2>/dev/null; done
sleep 3
rm -rf "$LOGS/camp" "$PROFILES"/check-*
exit "$code"
