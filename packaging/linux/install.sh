#!/bin/sh
# Installs or upgrades OurCamp Server as a systemd service (ADR-0043):
#   sudo ./install.sh [--host DOMAIN-OR-IP]
# Run from the unpacked archive. Settings in /etc/ourcamp/server.env survive upgrades.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
host=""
while [ $# -gt 0 ]; do
  case "$1" in
    --host)
      [ $# -ge 2 ] || { echo "install.sh: --host needs a value" >&2; exit 2; }
      host=$2
      shift 2
      ;;
    -h | --help)
      sed -n '2,4p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "install.sh: unknown argument $1" >&2
      exit 2
      ;;
  esac
done

[ "$(id -u)" -eq 0 ] || { echo "install.sh: run it as root (sudo ./install.sh)" >&2; exit 1; }
command -v systemctl >/dev/null 2>&1 || {
  echo "install.sh: needs systemd; without it run $here/oc-server directly" >&2
  exit 1
}

if ! id ourcamp >/dev/null 2>&1; then
  nologin=$(command -v nologin || echo /usr/sbin/nologin)
  useradd --system --user-group --no-create-home --home-dir /var/lib/ourcamp \
    --shell "$nologin" ourcamp
fi

install -d -m 0755 /opt/ourcamp
for file in oc-server livekit-server certbot-deploy-hook.sh; do
  install -m 0755 "$here/$file" "/opt/ourcamp/$file"
done
install -m 0755 "$here/ourcamp" /usr/local/bin/ourcamp

install -d -m 0755 /etc/ourcamp
if [ ! -f /etc/ourcamp/server.env ]; then
  install -m 0644 "$here/server.env" /etc/ourcamp/server.env
fi
if [ -n "$host" ]; then
  # Only the --host of the settings changes; the rest stays as the owner left it.
  current=$(sed -n 's/^OURCAMP_ARGS=//p' /etc/ourcamp/server.env)
  rest=$(printf '%s\n' "$current" | sed -E 's/(^| )--host [^ ]+//; s/^ +//')
  args=$(printf '%s %s' "--host $host" "$rest" | sed 's/ *$//')
  sed -i "s|^OURCAMP_ARGS=.*|OURCAMP_ARGS=$args|" /etc/ourcamp/server.env
fi

install -m 0644 "$here/ourcamp-server.service" /etc/systemd/system/ourcamp-server.service
systemctl daemon-reload
systemctl enable ourcamp-server >/dev/null
# An upgrade restarts onto the new binaries; a first install starts.
systemctl restart ourcamp-server

cat <<'DONE'
OurCamp Server запущен.
  sudo ourcamp status                        ссылка для друзей, каналы, люди
  sudo journalctl -u ourcamp-server -f       журнал
  /etc/ourcamp/server.env                    настройки (потом: sudo systemctl restart ourcamp-server)
Откройте в файрволе: TCP 7780 (сервер), TCP 7881 и UDP 7882 (голос).
DONE
