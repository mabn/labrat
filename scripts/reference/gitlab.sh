#!/usr/bin/env bash
# Runs a GitLab instance in Docker to record labrat's conformance fixtures against.
# All of its state lives in $GITLAB_HOME (config/, data/, logs/).
#
#   gitlab.sh start      create and start the container (pulls the image the first time)
#   gitlab.sh logs       follow the logs
#   gitlab.sh stop       stop it; the data stays on disk
#   gitlab.sh rm         stop and remove the container; the data stays on disk
#   gitlab.sh password   print the initial root password (deleted 24 hours after the first start)
#
# Settings (environment variables):
#   GITLAB_IMAGE_TAG   image tag, e.g. 18.11.14-ce.0 (required for start)
#   GITLAB_HOME        directory for the instance's state (required)
#   GITLAB_CONTAINER   container name (default: gitlab-ce)
#
# The ports are fixed: the recordings and the conformance harness assume
# http://localhost:8929 and SSH port 2424. So only one instance can run at a time.
set -euo pipefail

: "${GITLAB_HOME:?set GITLAB_HOME to the instance directory}"
NAME="${GITLAB_CONTAINER:-gitlab-ce}"
HTTP_PORT=8929
SSH_PORT=2424
HOST=localhost

# Applied when the container is created. After editing, run `gitlab.sh rm && gitlab.sh start`.
OMNIBUS_CONFIG="
external_url 'http://$HOST:$HTTP_PORT'
gitlab_rails['gitlab_shell_ssh_port'] = $SSH_PORT

# A single-user test instance doesn't need more.
puma['worker_processes'] = 0
sidekiq['concurrency'] = 10
prometheus_monitoring['enable'] = false
gitlab_kas['enable'] = false
"

case "${1:-start}" in
  start)
    mkdir -p "$GITLAB_HOME/config" "$GITLAB_HOME/logs" "$GITLAB_HOME/data"
    if docker container inspect "$NAME" >/dev/null 2>&1; then
      docker start "$NAME"
    else
      : "${GITLAB_IMAGE_TAG:?set GITLAB_IMAGE_TAG, e.g. 18.11.14-ce.0}"
      image="gitlab/gitlab-ce:$GITLAB_IMAGE_TAG"
      docker pull "$image"
      docker run --detach \
        --name "$NAME" \
        --hostname "$HOST" \
        --restart unless-stopped \
        --stop-timeout 300 \
        --shm-size 256m \
        --publish "$HTTP_PORT:$HTTP_PORT" \
        --publish "$SSH_PORT:22" \
        --env GITLAB_OMNIBUS_CONFIG="$OMNIBUS_CONFIG" \
        --volume "$GITLAB_HOME/config:/etc/gitlab" \
        --volume "$GITLAB_HOME/logs:/var/log/gitlab" \
        --volume "$GITLAB_HOME/data:/var/opt/gitlab" \
        "$image"
    fi
    echo "GitLab is starting at http://$HOST:$HTTP_PORT. It takes a few minutes."
    ;;
  logs) docker logs -f "$NAME" ;;
  # A long timeout: a PostgreSQL that is killed recovers very slowly on the next start.
  stop) docker stop -t 300 "$NAME" ;;
  rm) docker stop -t 300 "$NAME"; docker rm "$NAME" ;;
  password) docker exec "$NAME" grep 'Password:' /etc/gitlab/initial_root_password ;;
  *) echo "usage: $0 [start|logs|stop|rm|password]" >&2; exit 1 ;;
esac
