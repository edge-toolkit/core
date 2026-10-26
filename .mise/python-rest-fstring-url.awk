# Rewrites each path-parameter URL openapi-python-client builds with `str.format` as an f-string.
# The f-string is what DeepSource PYL-C0209 asks for, and ruff's UP032 will not produce it. The template quotes every
# parameter as `quote(str(<name>), safe="")`, so the placeholder alone rebuilds it; `sq` gives the f-string the
# single quotes it needs before Python 3.12.
BEGIN { sq = "'" }
/"url": "[^"]*"\.format\(/ {
  sub(/\.format\(.*$/, ",")
  sub(/"url": /, "&f")
  out = ""
  while (match($0, /\{[A-Za-z_][A-Za-z0-9_]*\}/)) {
    name = substr($0, RSTART + 1, RLENGTH - 2)
    out = out substr($0, 1, RSTART - 1) "{quote(str(" name "), safe=" sq sq ")}"
    $0 = substr($0, RSTART + RLENGTH)
  }
  $0 = out $0
}
{ print }
