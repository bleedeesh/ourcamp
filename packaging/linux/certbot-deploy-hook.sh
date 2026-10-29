#!/bin/sh
# certbot deploy hook (ADR-0042, ADR-0043): copies a fresh certificate where the
# ourcamp user can read it. /etc/letsencrypt is root's alone; the server rereads the
# files within a minute, without a restart.
#
#   sudo certbot certonly --standalone -d ourcamp.kz \
#     --deploy-hook /opt/ourcamp/certbot-deploy-hook.sh
set -eu

: "${RENEWED_LINEAGE:?run by certbot (--deploy-hook), which sets RENEWED_LINEAGE}"
dir=/etc/ourcamp/tls
install -d -m 0750 -o root -g ourcamp "$dir"
for file in privkey.pem fullchain.pem; do
  # A temporary file, then a rename: the server never reads half a file.
  install -m 0640 -o root -g ourcamp "$RENEWED_LINEAGE/$file" "$dir/.$file.new"
  mv -f "$dir/.$file.new" "$dir/$file"
done
echo "ourcamp: certificate copied to $dir"
