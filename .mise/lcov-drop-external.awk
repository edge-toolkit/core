# Keeps only the workspace records of an lcov file, dropping dependency and toolchain sources.
# A wasm covmap references every source its module linked, so dependency crates under ~/.cargo/registry and
# toolchain std under ~/.rustup ride along. Neither is in VCS and both skew the aggregate metric, so a record
# whose SF: path sits in one of those trees never reaches lcov.info.
{ buf = buf $0 ORS }
/^SF:/ { p = substr($0, 4); drop = (index(p, "/.cargo/") || index(p, "/.rustup/") || index(p, "/rustc/")) }
/^end_of_record$/ { if (!drop) printf "%s", buf; buf = ""; drop = 0 }
