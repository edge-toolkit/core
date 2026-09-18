# shellcheck shell=bash

# Turns a wasm-bindgen test build's instrumented .ll into the llvm-cov objects an export needs.
# Sourced by the coverage tasks that instrument a browser module, which differ only in which module they
# build: the caller passes the coverage directory, the llvm bin directory, and its own task name (which the
# failure message reports) as arguments, and reads the populated `objs` array afterwards. They arrive as
# arguments rather than as variables the caller happens to have set, so that what this fragment needs is
# declared at the call site instead of being an unwritten agreement about names.
#
# Both build-dir layouts are searched. A plain `deps/*.ll` glob broke when nightly made
# `-Z build-dir-new-layout` the default: per-crate intermediates moved to `<profile>/build/<pkg>/<hash>/out/`,
# the glob matched nothing, and the unexpanded pattern reached goawk as a literal filename -- `file
# "target/wasm32-unknown-unknown/debug/deps/*.ll" not found` on commit 12bfe419 at
# https://github.com/edge-toolkit/core/actions/runs/30797697402/job/91634929015. `find` also avoids the
# silent-nullglob trap the literal-pattern failure exposed.
covdir=$1
llbin=$2
task=$3
objs=()
lls="$(find target/wasm32-unknown-unknown/debug -path '*/build/*' -name '*.ll' 2>/dev/null)"
if [ -z "$lls" ]; then
  lls="$(find target/wasm32-unknown-unknown/debug/deps -name '*.ll' 2>/dev/null)"
fi
if [ -z "$lls" ]; then
  echo "$task: no instrumented .ll under target/wasm32-unknown-unknown/debug" >&2
  exit 1
fi
while IFS= read -r ll; do
  [ -n "$ll" ] || continue
  name="$(coreutils basename "$ll" .ll)"
  goawk -f .mise/llvm-cov-gut.awk "$ll" >"$covdir/$name.g.ll"
  "$llbin/llc" -filetype=obj -mtriple=x86_64-unknown-linux-gnu -o "$covdir/$name.o" "$covdir/$name.g.ll"
  objs+=("-object" "$covdir/$name.o")
done <<<"$lls"
