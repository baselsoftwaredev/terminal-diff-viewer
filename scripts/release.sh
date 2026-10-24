#!/bin/sh
# One entry point for local validation, versioning, and GitHub publishing.
set -eu
exec python3 "$(dirname "$0")/release.py" "$@"
