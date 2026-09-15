# Keeps only the lcov records whose SF: path contains the `want` substring, passed with `goawk -v want=...`.
# One substring per pass, which is all this filter can express: a caller that needs several source trees runs
# it once per tree and appends each result.
{ buf = buf $0 ORS }
/^SF:/ { keep = index($0, want) > 0 }
/^end_of_record$/ { if (keep) printf "%s", buf; buf = ""; keep = 0 }
