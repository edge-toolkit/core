# Prints one `IgnoredVulns` row per advisory id found in config/deny.toml.
# Both RUSTSEC-YYYY-NNNN and GHSA-xxxx-xxxx-xxxx ids are extracted, so a GHSA-only advisory (one RustSec has
# not assigned) still filters. An entry's `expires` date rides along as an `ignoreUntil` value.
BEGIN { q = "\"" }
{
  # An id can appear more than once on a line, and in prose as well as in an entry, so scan the whole line.
  # The expiry is read from the line as a whole: only a real ignore entry carries a `reason`, so a bare id
  # mentioned in a comment keeps the undated form and stays ignored indefinitely.
  rest = $0
  while (match(rest, /"(RUSTSEC-[0-9]{4}-[0-9]{4}|GHSA(-[0-9a-z]{4}){3})"/)) {
    # Save this match's span before any other match() call, which would overwrite RSTART/RLENGTH.
    # Advancing `rest` by the expiry match's span instead leaves the id still in the remainder, so the loop
    # matches it again and never terminates -- goawk spins on the first dated entry rather than failing.
    s = RSTART
    l = RLENGTH
    id = substr(rest, s + 1, l - 2)
    seen[id] = 1
    if (match($0, /reason = "expires [0-9]{4}-[0-9]{2}-[0-9]{2}"/)) {
      d = substr($0, RSTART, RLENGTH)
      sub(/.*expires /, "", d)
      sub(/"$/, "", d)
      expiry[id] = d
    }
    rest = substr(rest, s + l)
  }
}
END {
  for (id in seen) {
    if (id in expiry) {
      print "  { id = " q id q ", ignoreUntil = " expiry[id] " },"
    } else {
      print "  { id = " q id q " },"
    }
  }
}
