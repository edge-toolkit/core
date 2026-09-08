//! Emit `ET_MATH1_INPUT_PATH` so `include_str!` in `src/lib.rs` embeds the canonical math1 input -- the same
//! bytes every math1 test harness injects.

fn main() {
    et_path::emit_repo_file_env("ET_MATH1_INPUT_PATH", "services/ws-test-server/data/math1-input.json");
}
