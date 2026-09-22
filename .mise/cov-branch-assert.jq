# Names the libs/ crates an llvm-cov JSON summary shows as wrong.
# Wrong means one of two things: a crate in $libs that is not at 100% branch coverage, or a crate in $nocode
# that the same summary shows any records for at all.
# That JSON (`--format=text`, confusingly) carries a per-file `summary.branches` object, so `.data[0].files[]`
# already holds everything this reads. A crate with no records at all is reported rather than passing
# vacuously: a crate dropping out of the report is how coverage silently stops being measured.
# $nocode inverts that for the crates that are constants and macros only, which llvm-cov cannot record even
# when their tests run -- an empty report is expected there, and records appearing mean the crate grew code.
($libs | split(" ") | map(select(length > 0))) as $names
| ($nocode | split(" ") | map(select(length > 0))) as $constants
| .data[0].files as $files
| [ ( $names[]
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
        end ),
    ( $constants[]
      | . as $n
      | ("/libs/" + $n + "/") as $dir
      | ($files | map(select(.filename | contains($dir)))) as $hit
      | if ($hit | length) == 0
        then empty
        else "libs/" + $n + ": has coverable code now -- move it to native_cov_libs and cover it"
        end ) ]
| .[]
