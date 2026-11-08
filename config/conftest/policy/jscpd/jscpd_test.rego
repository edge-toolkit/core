# Unit tests for the jscpd baseline ratchet, run by `conftest verify`.
#
# Each case pins the ceiling itself, so moving declared_clones never invalidates a case; the committed value is
# held to the step rule by the first case alone.
package jscpd_test

import data.jscpd

# A baseline recording `total` distinct clone fingerprints, in the shape jscpd writes.
baseline(total) := {"fingerprints": {$"fingerprint-{n}": {} | some n in numbers.range(1, total)}}

test_the_committed_ceiling_is_a_whole_step if {
	jscpd.declared_clones % jscpd.ceiling_step == 0
}

test_a_baseline_at_the_ceiling_passes if {
	count(jscpd.deny) == 0 with input as baseline(380) with jscpd.declared_clones as 380
}

test_a_baseline_over_the_ceiling_is_refused if {
	msgs := jscpd.deny with input as baseline(381) with jscpd.declared_clones as 380
	count(msgs) == 1
	some msg in msgs
	contains(msg, "has 381 clones, over the 380")
}

test_a_baseline_too_far_under_the_ceiling_is_refused if {
	msgs := jscpd.deny with input as baseline(340) with jscpd.declared_clones as 380
	count(msgs) == 1
	some msg in msgs
	contains(msg, "40 under the declared 380")
}

test_a_ceiling_off_the_step_is_refused if {
	msgs := jscpd.deny with input as baseline(357) with jscpd.declared_clones as 357
	count(msgs) == 1
	some msg in msgs
	contains(msg, "declared_clones is 357, which is not a multiple of 10")
}
