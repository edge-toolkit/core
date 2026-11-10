# File paths for the conftest policies, spelled the same way on every OS.
#
# conftest reports each path with the native separator of the OS it runs on, so a Windows lane hands over
# `.github\workflows\x.yaml` where every rule is written against `.github/workflows/x.yaml`. A rule that matches on
# the raw path matches nothing there, and that fails open: the namespace reports clean on Windows while checking
# nothing. Every policy run with `--combine` therefore iterates `files` here rather than `input` itself, so no rule
# ever sees a backslash.
package paths

# A path with its separators turned into forward slashes.
normalised(path) := replace(path, "\\", "/")

# The files exactly as conftest hands them over, read here and nowhere else.
raw_files := input

# Every file of a `--combine` input, as conftest hands it over but with its path normalised.
files := [object.union(file, {"path": normalised(file.path)}) | some file in raw_files]
