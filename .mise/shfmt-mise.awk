# Split the multi-line bash bodies out of a `.mise/config*.toml` so shfmt can format them, then merge back.
#
# `-v mode=split` writes the Nth body of the config to <dir>/NNN.sh, plus <dir>/NNN.tok holding the Tera
# tokens it masked out of that body (one per line, in the order they appeared). `-v mode=merge` re-reads the
# same config and re-emits it on stdout with each body replaced by the (by then shfmt-formatted) <dir>/NNN.sh.
# Bodies are paired between the two passes positionally, so both passes must see the same input file.
#
# A body is the run of lines between a `run = """` (or `run = '''`) line and the matching closing delimiter on
# its own line -- the shape every multi-line mise task uses, and the only shape a task `run` can take given the
# conftest rule that a multiline run is a bash script. Only the opening delimiter closes a body, so a `"""`
# python docstring inside a `'''` literal block does not end it early.
#
# Two encodings sit between the TOML byte and the byte shfmt must see, and both are reversed on the way back:
#
#   Tera     `{{ ... }}` / `{% ... %}` masks to `ET_TERA_<n>_`, a plain bash word shfmt leaves alone. Without
#            it a `{% if os() == 'windows' %}` body fails to parse (`(` where shfmt wants a word) and shfmt
#            declines to format it at all. Masking happens before the escape decode and unmasking after the
#            re-encode, so a token's own bytes round-trip verbatim however they were spelled.
#   escapes  A `"""` block is a TOML *basic* string, so `\\` and `\"` in the file are one backslash and one
#            quote by the time the shell sees them; shfmt has to be handed that decoded form (raw `\\(` reads
#            as escaped-backslash plus a bare `(`, which does not parse), and the merge pass re-escapes each
#            backslash on the way out. The decode deliberately rejects every other escape rather than growing
#            a table for them: a lone `\n` / `\r` / `\b` in a body is nearly always meant as the two
#            characters the shell needs, and TOML quietly folding it into a control character is a bug in the
#            body (a `\b` inside a comment here once decoded to a backspace and ate the surrounding text).
#            `'''` blocks are TOML *literal* strings -- no escape processing at all -- so they pass untouched.

function fail(msg) {
  printf("%s:%d: %s\n", FILENAME, FNR, msg) > "/dev/stderr"
  exit 1
}

function placeholder(n) {
  return "ET_TERA_" n "_"
}

# Replace every occurrence of `from` with `to`, byte-wise.
# index/substr rather than sub(): neither argument is a regex, and `&` in an awk replacement is magic.
function subst_all(s, from, to, out, p) {
  out = ""
  while ((p = index(s, from)) > 0) {
    out = out substr(s, 1, p - 1) to
    s = substr(s, p + length(from))
  }
  return out s
}

# Swap each Tera token for its placeholder, recording the original in the body's .tok sidecar.
function mask(line, out, rest, tok) {
  out = ""
  rest = line
  while (match(rest, /[{][{][^}]*[}][}]|[{][%][^%]*[%][}]/)) {
    tok = substr(rest, RSTART, RLENGTH)
    ntok++
    print tok > tokfile
    out = out substr(rest, 1, RSTART - 1) placeholder(ntok)
    rest = substr(rest, RSTART + RLENGTH)
  }
  return out rest
}

function unmask(line, i) {
  for (i = 1; i <= ntok; i++) {
    line = subst_all(line, placeholder(i), tok[i])
  }
  return line
}

# Undo TOML basic-string escaping, one left-to-right pass so `\\"` decodes as backslash then quote.
function decode(s, out, n, i, c, nx) {
  out = ""
  n = length(s)
  for (i = 1; i <= n; i++) {
    c = substr(s, i, 1)
    if (c != "\\") {
      out = out c
      continue
    }
    nx = substr(s, i + 1, 1)
    if (nx == "\\" || nx == "\"") {
      out = out nx
      i++
      continue
    }
    fail("unsupported TOML escape \"\\" nx "\" in a task body; double the backslash to pass it to the shell")
  }
  return out
}

function encode(s) {
  return subst_all(s, "\\", "\\\\")
}

BEGIN {
  if (mode != "split" && mode != "merge") {
    printf("shfmt-mise.awk: -v mode= must be split or merge\n") > "/dev/stderr"
    exit 1
  }
  if (dir == "") {
    printf("shfmt-mise.awk: -v dir= must name the body directory\n") > "/dev/stderr"
    exit 1
  }
}

!inbody && ($0 == "run = \"\"\"" || $0 == "run = '''") {
  inbody = 1
  delim = ($0 == "run = \"\"\"") ? "\"\"\"" : "'''"
  basic = (delim == "\"\"\"")
  idx++
  body = sprintf("%s/%03d.sh", dir, idx)
  tokfile = sprintf("%s/%03d.tok", dir, idx)
  ntok = 0
  if (mode == "split") {
    printf("") > body
    printf("") > tokfile
    next
  }
  print
  while ((getline line < tokfile) > 0) {
    tok[++ntok] = line
  }
  close(tokfile)
  while ((getline line < body) > 0) {
    if (basic) {
      line = encode(line)
      if (index(line, "\"\"\"") > 0) {
        fail("formatted body contains `\"\"\"`, which would close the TOML string early")
      }
    }
    print unmask(line)
  }
  close(body)
  next
}

inbody && $0 == delim {
  inbody = 0
  if (mode == "split") {
    close(body)
    close(tokfile)
    next
  }
  print
  next
}

inbody {
  if (mode == "merge") {
    next
  }
  line = mask($0)
  if (basic) {
    line = decode(line)
  }
  print line > body
  next
}

mode == "merge" {
  print
}
