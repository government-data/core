#!/bin/sh
# In development the applications' stacks (infra/<app>/compose.dev.yml) join
# the network namespace of the application's Traefik. When `task dev`
# recreates Traefik, they are left in a namespace that no longer exists:
# Traefik answers 502 for RustFS and Grafana. This recreates, of those
# already created, the ones joined to another Traefik.
set -eu
cd "$(dirname "$0")/../.."

traefik="${APP_STACK:-data-quality}-traefik-1"
current=$(docker inspect -f '{{.Id}}' "$traefik")

# project stack-directory
stacks="lake rustfs
dolt dolt
observability observability"

echo "$stacks" | while read -r project dir; do
  stale=0
  for c in $(docker ps -aq --filter "label=com.docker.compose.project=data-quality-$project"); do
    mode=$(docker inspect -f '{{.HostConfig.NetworkMode}}' "$c")
    case "$mode" in
      "container:$current") ;;
      container:*) stale=1 ;;
    esac
  done
  [ "$stale" = 1 ] || continue

  echo "data-quality-$project: joining the new Traefik"
  docker compose --project-directory . --env-file .env -p "data-quality-$project" \
    -f "infra/$dir/docker-compose.yml" -f "infra/$dir/compose.dev.yml" \
    up -d --force-recreate
done
