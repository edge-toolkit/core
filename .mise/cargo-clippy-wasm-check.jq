# The workspace crates `cargo-clippy-wasm-check` lints as the browser builds them, as `-p <name>` words.
# Reads `cargo metadata --no-deps`: a browser crate depends on one of `$names`, and the crates picked are the
# workspace crates a browser crate depends on that are not browser crates themselves.
[.packages[].name] as $ws
| [.packages[] | select(any(.dependencies[].name; . as $dep | any($names[]; . == $dep)))] as $browser
| [$browser[].dependencies[] | select(.kind == null) | .name]
| unique
| .[]
| select(. as $name | any($ws[]; . == $name) and all($browser[].name; . != $name))
| "-p", .
