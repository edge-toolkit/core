# Ratchet on the jscpd duplication baseline, evaluated per file.
# Run with `--namespace jscpd`; the conftest-check-jscpd task feeds config/jscpd-baseline.json on its own, so
# `input` is that file's parsed contents directly rather than a --combine array.
#
# jscpd-check runs `--fail-on-new-clones` against that baseline, so every fingerprint it records is one clone the
# build no longer complains about. The file is therefore a debt ledger, and the point of jscpd is to pay the debt
# down: entries should only ever leave it as duplication is factored away. Nothing in jscpd itself enforces that
# direction, and `jscpd-baseline-update` will happily rewrite the file to bless whatever duplication exists today
# -- which silences the finding in front of you and every future one that lands in the same shape.
#
# So the count lives here as well, as a ceiling the baseline may not exceed. Growing the ledger without raising
# this number fails the build, which turns a regenerated baseline from an invisible chore into a one-line diff a
# reviewer can see and question.
#
# The ceiling carries a bounded slack so routine cleanups do not each demand a policy edit: the baseline may sit
# up to max_slack under it. Past that the policy fails too, which stops headroom accumulating into a cushion a
# later increase could hide under -- the failure there is not a problem, it is the prompt to bank the progress by
# lowering the ceiling.
#
# Lowering this number is ordinary work -- remove duplication, then bring it down. RAISING it is not: it means
# duplication was accepted rather than fixed, so it needs the operator's explicit sign-off, exactly as deleting a
# lint rule does.
package jscpd

# Ceiling on the clone fingerprints recorded in config/jscpd-baseline.json.
# May only ever be lowered. See the header for what raising it implies.
declared_clones := 359

# How far under the ceiling the baseline may sit before the ceiling has to be banked downward.
max_slack := 20

# Module-scope sprintf templates, each on its own line so the source stays inside the line limit.
grew_msg := "jscpd baseline has %d clones, over the %d the policy allows -- a baseline may only shrink, never grow"

slack_msg := "jscpd baseline has %d clones, %d under the declared %d -- lower declared_clones to bank the progress"

baseline_clones := count(object.keys(input.fingerprints))

deny contains msg if {
	baseline_clones > declared_clones
	msg := sprintf(grew_msg, [baseline_clones, declared_clones])
}

deny contains msg if {
	slack := declared_clones - baseline_clones
	slack > max_slack
	msg := sprintf(slack_msg, [baseline_clones, slack, declared_clones])
}
