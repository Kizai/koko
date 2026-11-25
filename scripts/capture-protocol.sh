#!/bin/bash
set +x
set -euo pipefail
umask 077
cd "$(dirname "$0")/.."
exec python3 -B scripts/capture_session.py "$@"
