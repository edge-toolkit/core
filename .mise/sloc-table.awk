# Prints the per-language tsv as an aligned table, adding each language's test share and a closing total.
# The share is derived here rather than stored, so the tsv holds only measured counts.
function share(part, whole) { return (whole > 0 ? sprintf("%.1f", 100 * part / whole) : "-") }
BEGIN { FS = "\t"; row = "%-30s %10d %10d %10d %7s\n" }
NR == 1 { printf "%-30s %10s %10s %10s %7s\n", "Language", $2, $3, $4, "test%"; next }
{
  nt += $2
  t += $3
  total += $4
  printf row, substr($1, 1, 30), $2, $3, $4, share($3, $4)
}
END { printf row, "TOTAL", nt, t, total, share(t, total) }
