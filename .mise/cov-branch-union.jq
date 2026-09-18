# Folds several llvm-cov JSON exports into the one summary cov-branch-assert.jq reads.
# The exports (`--format=text`, with branches) arrive as `inputs`, one per coverage source: each WASI guest, the
# browser agent, pic-viewer. A source file that is linked into several of them -- libs/web, libs/wasi-guest --
# shows up in each with only the outcomes that source happened to exercise, so the outcomes are unioned by
# region across every export: an arm is covered if anything, anywhere, took it, which is what "did anything
# exercise this branch" means. A file without branches still gets a 0/0 entry, so a crate that was compiled but
# has nothing to branch on reads as present rather than as never exercised.
#
# A branch entry is llvm-cov's [line_start, col_start, line_end, col_end, count, false_count, file_id,
# expansion_id, kind]; the first four name the region and the next two are its two outcomes.
[ inputs | .data[0].files[] | { filename, branches: (.branches // []) } ]
| group_by(.filename)
| map(
    ([ .[].branches[] | { region: (.[0:4] | map(tostring) | join(":")), taken: (.[4] > 0), skipped: (.[5] > 0) } ]
      | group_by(.region)
      | map({ taken: (map(.taken) | any), skipped: (map(.skipped) | any) })) as $regions
    | { filename: .[0].filename,
        summary: { branches: {
          count: ($regions | length * 2),
          covered: ($regions | map((if .taken then 1 else 0 end) + (if .skipped then 1 else 0 end)) | add // 0)
        } } })
| { data: [ { files: . } ] }
