# Guts every function body in an instrumented .ll, keeping only the signatures and the __llvm_covmap records.
# llvm-cov cannot read a coverage map from a .wasm, so the gutted module is compiled to a fixed x86_64 ELF
# object instead; the hit counts come from the .profraw, so the object needs no code. The target-cpu and
# target-features attributes go with the bodies: browser .ll carries a wasm `mvp` cpu and wasm features that
# make llc reject the x86_64 target with `64-bit code requested on a subtarget that doesn't support it`. The
# WASI guest .ll carries neither attribute, so there the strip is a no-op. An attribute group left empty by it
# is refilled with `nounwind`, which llc parses; an empty `{ }` group it rejects.
/^target datalayout/ { next }
/^target triple/ { next }
/^define/ { print; print "start:"; print "  unreachable"; print "}"; skip = 1; next }
skip && /^}/ { skip = 0; next }
skip { next }
{
  gsub(/"target-cpu"="[^"]*"/, "")
  gsub(/"target-features"="[^"]*"/, "")
  if ($0 ~ /^attributes #/ && $0 ~ /\{[[:space:]]*\}/) sub(/\{[[:space:]]*\}/, "{ nounwind }")
  print
}
