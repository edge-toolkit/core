# Names every libs/ crate in $libs that an llvm-cov JSON summary does not show at 100% branch coverage.
# That JSON (`--format=text`, confusingly) carries a per-file `summary.branches` object, so `.data[0].files[]`
# already holds everything this reads. A crate with no records at all is reported rather than passing
# vacuously: a crate dropping out of the report is how coverage silently stops being measured.
($libs | split(" ")) as $names
| .data[0].files as $files
| [ $names[]
    | . as $n
    | ("/libs/" + $n + "/") as $dir
    | ($files | map(select(.filename | contains($dir)))) as $hit
    | if ($hit | length) == 0
      then "libs/" + $n + ": no records in the report -- this run never exercised the crate"
      else ($hit[]
            | select(.summary.branches.covered < .summary.branches.count)
            | "libs/" + $n + ": " + .filename + " "
              + (.summary.branches.covered | tostring) + "/"
              + (.summary.branches.count | tostring) + " branches")
      end ]
| .[]
