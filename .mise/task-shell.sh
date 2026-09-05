#!/usr/bin/env sh
# Shell wrapper every mise task runs its body through, so the body sees a PATH its shell can actually use.
#
# mise rewrites a Windows task environment's PATH into msys form (`/c/x`) while still joining the entries with
# Windows' `;` (jdx/mise#12173, first released in 2026.8.9). busybox-w32 ash -- what MISE_BASH_PATH points at on
# Windows -- resolves drive-letter paths only, and splits PATH on `:` alone, so it reads that whole string as one
# unusable entry and every tool disappears at once:
#
#   c:/i/http-busybox/1.37.0/ash: line 0: cargo: not found
#
# with dart, uv, rclone, zig, wasm-pack and coreutils all failing the same way in the same run, on commit
# 17056fad10ade2af7b2df3eced29f9ae77d20f3e at
# https://github.com/edge-toolkit/core/actions/runs/33877826343/job/101038940930. Upstream deleted the rewrite in
# jdx/mise#12696, which sits in no release yet -- once it ships, this file becomes a no-op on every platform and
# can go. Repointing MISE_BASH_PATH at an msys2 bash would also fix it, since that runtime renormalises the
# environment on startup, but it costs the cygheap fork pathology busybox was adopted to escape.
#
# Rewriting `/c/x` to `c:/x` is the whole repair -- swapping the separator alone does nothing, because busybox
# resolves no msys-form entry under either separator. mise appends the task body as the argument after the shell
# operands, so it arrives here as `$1`; a startup file is not an option, as ash sources `$ENV` only when
# interactive. Both guards keep this inert wherever the mangling cannot apply: off Windows, and on a PATH that
# is not `;`-joined.
case ${OS:-} in
Windows_NT)
  case $PATH in
  *";"*)
    saved_ifs=$IFS
    IFS=";"
    fixed=
    for entry in $PATH; do
      case $entry in
      /?/*)
        rest=${entry#/}
        drive=${rest%%/*}
        rest=${rest#*/}
        entry="$drive:/$rest"
        ;;
      esac
      fixed="${fixed:+$fixed;}$entry"
    done
    IFS=$saved_ifs
    PATH=$fixed
    export PATH
    ;;
  esac
  ;;
esac

eval "$1"
