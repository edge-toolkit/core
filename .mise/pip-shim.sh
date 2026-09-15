#!/bin/sh
# `bin/pip` for a pipx shared-libs venv created with `--without-pip`, alongside the dropped-in site-packages.
exec "$(dirname "$0")/python" -m pip "$@"
