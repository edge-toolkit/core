# Lists each entry of a mise lockfile on a line of its own: its `[[tools.<name>]]` header, version and backend.
# mise-lock-check sorts and compares these lists, so each value stays tied to the tool it belongs to while the order
# the entries were written in counts for nothing. Only an entry's own `version` and `backend` are taken, the first of
# each after its header, so a later table of that tool -- its options, a platform's checksum -- adds nothing.
/^\[\[tools\./ {
    if (tool != "") print tool " " version " " backend
    tool = $0
    version = ""
    backend = ""
}
/^version = / && version == "" { version = $0 }
/^backend = / && backend == "" { backend = $0 }
END { if (tool != "") print tool " " version " " backend }
