#!/bin/bash
set +x
set -euo pipefail
umask 077
exec python3 -B "$(dirname "$0")/capture_session.py" "$@"
