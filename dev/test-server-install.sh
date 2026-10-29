#!/bin/sh
# Installs the Linux server archive on this machine under systemd and checks it end to
# end (ADR-0043): CI's server-linux job runs it on a throwaway runner.
#   dev/test-server-install.sh target/pack/ourcamp-server-…-linux-x64.tar.gz
# Not for a machine you care about: it installs ourcamp-server for real.
set -eu

archive=$1
fail() {
  echo "FAIL: $*" >&2
  sudo journalctl -u ourcamp-server --no-pager -n 80 >&2 || true
  exit 1
}

work=$(mktemp -d)
tar xzf "$archive" -C "$work"
dir=$(find "$work" -mindepth 1 -maxdepth 1 -type d)

echo "--- the server is one static binary"
file "$dir/oc-server" | grep -qE "statically linked|static-pie linked" || fail "oc-server is not static: $(file "$dir/oc-server")"

echo "--- install, with the address friends use"
sudo "$dir/install.sh" --host 127.0.0.1
grep -q '^OURCAMP_ARGS=--host 127.0.0.1$' /etc/ourcamp/server.env || fail "--host not in server.env"

echo "--- it comes up: the owner's status answers"
up=""
for _ in $(seq 1 30); do
  if status=$(sudo ourcamp status 2>/dev/null); then up=yes; break; fi
  sleep 1
done
[ -n "$up" ] || fail "no status within 30 s"
printf '%s\n' "$status"
printf '%s\n' "$status" | grep -q "Ссылка: ourcamp://127.0.0.1/join/" || fail "no friends link in status"
pgrep -u ourcamp -x livekit-server >/dev/null || fail "its LiveKit is not running"

echo "--- the owner's commands work"
sudo ourcamp channel add Проверка | grep -q "Проверка" || fail "channel add"
sudo ourcamp status | grep -q "Проверка" || fail "the new channel is not in status"
if ourcamp --bind 0.0.0.0:1 2>/dev/null; then fail "flags must not start a second server"; fi

echo "--- an upgrade keeps the settings and the camp"
sudo sed -i 's|^OURCAMP_ARGS=.*|OURCAMP_ARGS=--host 127.0.0.1 --open|' /etc/ourcamp/server.env
sudo "$dir/install.sh"
grep -q -- '--open' /etc/ourcamp/server.env || fail "the upgrade overwrote server.env"
up=""
for _ in $(seq 1 30); do
  if status=$(sudo ourcamp status 2>/dev/null); then up=yes; break; fi
  sleep 1
done
[ -n "$up" ] || fail "no status after the upgrade"
printf '%s\n' "$status" | grep -q "Проверка" || fail "the camp was lost on upgrade"
printf '%s\n' "$status" | grep -q "открытый" || fail "--open from server.env not applied"

echo "--- systemctl stop is a clean stop"
sudo systemctl stop ourcamp-server
sudo test ! -e /var/lib/ourcamp/control.json || fail "control.json left behind"
if pgrep -x livekit-server >/dev/null; then fail "LiveKit outlived the server"; fi
sudo journalctl -u ourcamp-server --no-pager | grep -q "\[oc-server\] stopping" || fail "not stopped by its own handler"

echo "OK"
