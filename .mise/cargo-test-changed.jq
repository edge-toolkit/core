# The `-p <spec>` arguments of the plan's test selection, less each package the platform's excludes name.
# `$ex` is the excludes var as given, a space-joined `--exclude <package>` list; a spec may carry an `@<version>`.
.work.nextest.scope.selection.cargo_args |
    [range(0; length; 2) as $i | .[$i:$i + 2]] |
    map(select(.[1] | split("@")[0] as $p | [$ex | split(" ") | .[] | select(. == $p)] | length == 0)) |
    flatten |
    .[]
