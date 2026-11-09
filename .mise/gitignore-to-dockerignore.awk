# Rewrites .gitignore patterns as .dockerignore ones, which Docker anchors differently.
# A bare .dockerignore pattern is root-anchored, whereas a slash-less .gitignore pattern matches at any depth, so a
# pattern whose only slash is a trailing one gains a `**/` prefix. A leading slash anchors a pattern in both, and is
# dropped; a slash anywhere else anchors it in both as written. A `!` negation stays in front of whichever form
# results, and comments and blank lines pass through unchanged.
/^[[:space:]]*#/ { print; next }
/^[[:space:]]*$/ { print; next }
{
  neg = ""
  line = $0
  if (substr(line, 1, 1) == "!") { neg = "!"; line = substr(line, 2) }
  if (substr(line, 1, 1) == "/") { print neg substr(line, 2); next }
  core = line
  sub(/\/$/, "", core)
  if (index(core, "/") > 0) print neg line
  else print neg "**/" line
}
