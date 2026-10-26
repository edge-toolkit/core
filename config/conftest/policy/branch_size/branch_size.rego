# The files a branch modifies, held to the most CodeRabbit will review in one pull request.
# Run with `--combine --parser ignore --namespace branch_size` over the changed-file list on stdin, one path per
# line, so `input` is a single entry whose `contents` are those lines.
#
# CodeRabbit reviews nothing at all in a pull request that modifies more than 100 files, and says so only in a
# skipped-review note, so a branch past the limit loses its review without any check going red. Split the work into
# branches that each stay at or under it.
package branch_size

# The most files a pull request may modify and still be reviewed by CodeRabbit.
max_files := 100

too_many_msg := "this branch modifies %d files, over the %d CodeRabbit reviews in one pull request -- split it up"

# Every path in the list, blank lines aside.
changed contains line.Original if {
	some file in input
	some line in file.contents
	line.Original != ""
}

# A branch may modify at most `max_files` files, or CodeRabbit skips its pull request's review.
deny contains msg if {
	count(changed) > max_files
	msg := sprintf(too_many_msg, [count(changed), max_files])
}
