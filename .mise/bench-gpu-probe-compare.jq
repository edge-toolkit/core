# Every way a measured et-gpu-probe bench results file differs from the recorded one, one line each.
# `$r` is the recorded file, `$m` the measured one, and `$f` the factor a latency may be slower or faster by.
($r.benchmarks | keys) as $rk | ($m.benchmarks | keys) as $mk
| (($rk - $mk)[] | "\(.): recorded, but not measured"),
  (($mk - $rk)[] | "\(.): measured, but not recorded"),
  ($rk[] | select($m.benchmarks[.] != null) | . as $id
    | $r.benchmarks[$id].latency.value as $was | $m.benchmarks[$id].latency.value as $now
    | select($was > 0 and $now > 0 and ($now / $was > $f or $was / $now > $f))
    | "\($id): \($now) ns, against \($was) ns recorded")
