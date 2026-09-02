# Bidirectional cross-reference between config/generated-trees.toml and the linter configs it governs.
# Run with `--namespace generated_trees`; the conftest-check-generated-trees task feeds the declaration plus every
# governed config to one `--combine` evaluation, letting a rule compare across files.
#
# Forward: a config listed in a tree's `required_in` must actually carry that tree's exclusion, or the linter walks
# generator output and reports findings nobody can fix by hand. Reverse: a config carrying a tree exclusion must be
# listed, or the declaration is no longer the whole picture and the next tree added will miss that config.
#
# Exclusions are matched as exact strings rather than by substring, because a wrong path base is the failure this
# policy exists to catch: semgrep silently matches nothing when a `paths.exclude` entry loses its leading slash,
# and the mistake is invisible until generator output shows up in a report.
package generated_trees

source_path := "config/generated-trees.toml"

# Path separators, normalised so a Windows lane compares equal to the declaration.
# conftest reports each path the way the OS handed it over, so on Windows a file found by walking a directory
# arrives as `config\semgrep\x.yaml` while one named directly on the command line keeps the `/` it was typed with.
# Both must match the forward-slash paths the declaration uses, or every forward check fails on that lane while
# the reverse checks stay silent -- exactly the shape the Windows job showed.
normalised(path) := replace(path, "\\", "/")

is_source(file) if normalised(file.path) == source_path

# Module-scope sprintf templates each live on their own line so the source stays readable.
missing_msg := "%s: missing the %q exclusion for the %q generated tree, which config/generated-trees.toml requires"

undeclared_msg := "%s: carries the %q exclusion for the %q generated tree but is not in its `required_in` list"

no_anchor_msg := "config/generated-trees.toml: [tree.%s] requires %q, which matches no [anchor] key"

# The declared trees, keyed by name.
trees[name] := tree if {
	some file in input
	is_source(file)
	some name, tree in file.contents.tree
}

# The declared per-tool path templates, keyed by config path or path prefix.
anchors[prefix] := template if {
	some file in input
	is_source(file)
	some prefix, template in file.contents.anchor
}

# The exclusion string `config` must use for `tree`, resolved through the [anchor] table.
expected(config, tree) := pattern if {
	some prefix, template in anchors
	startswith(config, prefix)
	pattern := replace(template, "{tree}", tree)
}

# Every string value anywhere in a governed config's parsed contents, paired with the file it came from.
# A plain walk keeps the policy free of per-tool schema knowledge -- semgrep nests exclusions under
# `rules[].paths.exclude`, ast-grep under `files`, jscpd under `ignore`, pyrefly under `project-excludes`.
config_strings contains [normalised(file.path), value] if {
	some file in input
	not is_source(file)
	walk(file.contents, [_, value])
	is_string(value)
}

# The patterns `config` is expected to carry for `tree`: the narrower set when one is declared, else the whole tree.
allowed(config, name) := trees[name].narrower[config]

allowed(config, name) := [expected(config, name)] if {
	not trees[name].narrower[config]
}

# The whole-tree pattern with its trailing `**` removed, used to spot any exclusion aimed into the tree.
tree_prefix(config, name) := trim_suffix(expected(config, name), "**")

accounted(config, value, name) if {
	config in trees[name].required_in
	value in allowed(config, name)
}

deny contains msg if {
	some name, tree in trees
	some config in tree.required_in
	some pattern in allowed(config, name)
	not [config, pattern] in config_strings
	msg := sprintf(missing_msg, [config, pattern, name])
}

# Reverse direction, matched on the tree prefix rather than on the exact pattern.
# That way a narrowed exclusion cannot slip in undeclared either -- an unlisted `/verification/**/foo.yaml` is as
# much a drift as an unlisted whole-tree skip.
deny contains msg if {
	some pair in config_strings
	some name, _ in trees
	startswith(pair[1], tree_prefix(pair[0], name))
	not accounted(pair[0], pair[1], name)
	msg := sprintf(undeclared_msg, [pair[0], pair[1], name])
}

# A `required_in` entry that no [anchor] key covers would make the forward rule above vacuous.
# Fail loudly instead: there is no way to know what exclusion string that config should carry.
deny contains msg if {
	some name, tree in trees
	some config in tree.required_in
	not expected(config, name)
	msg := sprintf(no_anchor_msg, [name, config])
}
