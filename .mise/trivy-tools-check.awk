# Print the install path of each mise tool whose config lives under this repository, for trivy-tools-check.
# Input is `install_path<TAB>source.path` per tool; ENVIRON["repo_root"] is mise's config_root. Every path is turned
# to forward slashes before the comparison, since on Windows mise reports `C:\...` while the bash globs that consume
# this output produce `C:/...`. The root comes from the environment rather than `-v`, which would read the
# backslashes in a Windows path as escape sequences.
BEGIN {
  FS = "\t"
  root = ENVIRON["repo_root"] "/"
  gsub(/\\/, "/", root)
}

{
  gsub(/\\/, "/")
}

index($2, root) == 1 {
  print $1
}
