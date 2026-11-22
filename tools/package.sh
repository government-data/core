#!/usr/bin/env bash
# The core's package for quality-sdk (decision 0080, point 6):
#
#   tools/package.sh VERSION        # VERSION must be version.txt's
#
# writes quality-data-VERSION.tar.gz (in OUT_DIR, default dist/) with, at
# its root:
#
#   module.yaml   the manifest, its version VERSION
#   compose.yml   infra/package/compose.yml
#   infra/        the files that compose includes and mounts (Traefik's
#                 static and dynamic configuration, its machine
#                 certificate's entrypoint, PostgreSQL's init and backup
#                 scripts and its TLS overlay) and the installer's one-shot
#                 overlays
#
# The files are taken from the commit (HEAD), never the working tree, so
# nothing per installation (internal.yml's copies, certificates, secrets,
# .env) can come in: those are git-ignored. Regular files and directories
# only, as quality-sdk's package reader accepts; owner root, the commit's
# time, sorted, so the same commit makes the same files.
set -euo pipefail
export LC_ALL=C
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="${1:-}"
want="$(tr -d '[:space:]' <version.txt)"
if [[ -z "$version" ]]; then
  echo "usage: tools/package.sh VERSION (version.txt has $want)" >&2
  exit 2
fi
if [[ "$version" != "$want" ]]; then
  echo "VERSION $version is not version.txt's ($want)" >&2
  exit 2
fi

files=(
  module.yaml
  infra/package/compose.yml
  infra/traefik/docker-compose.yml
  infra/traefik/certificate.yml
  infra/traefik/traefik.yml
  infra/traefik/internal.yml
  infra/traefik/machine/entrypoint.sh
  infra/traefik/dynamic
  infra/postgres/docker-compose.yml
  infra/postgres/api.override.yml
  infra/postgres/tls.yml
  infra/postgres/tls
  infra/postgres/initdb
  infra/postgres/scripts
  infra/postgres/app-machine.yml
  infra/postgres/updater-certificate.yml
)

out="${OUT_DIR:-dist}"
mkdir -p "$out"
stage="$(mktemp -d)"
trap 'rm -rf -- "${stage:?}"' EXIT
git archive --format=tar HEAD -- "${files[@]}" | tar -x -C "$stage"

mv "$stage/infra/package/compose.yml" "$stage/compose.yml"
rmdir "$stage/infra/package"
grep -q '^version: 0\.0\.0$' "$stage/module.yaml" || {
  echo "module.yaml has no 'version: 0.0.0' line to fill" >&2
  exit 1
}
sed -i "s/^version: 0\.0\.0\$/version: $version/" "$stage/module.yaml"

# The package's modes, not this machine's umask: read by all, the scripts
# executable.
chmod -R u=rwX,go=rX "$stage"

bad="$(find "$stage" ! -type f ! -type d)"
if [[ -n "$bad" ]]; then
  echo "not a file or a directory: ${bad//$stage\//}" >&2
  exit 1
fi

when="$(git log -1 --format=%cI HEAD)"
package="$out/quality-data-$version.tar.gz"
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="$when" \
  --format=gnu -cf - . | gzip -n >"$package.partial"
mv -f "$package.partial" "$package"
echo "$package ($(git rev-parse --short HEAD)):"
tar -tzf "$package" | grep -v '/$' | sed 's|^\./|  |'
