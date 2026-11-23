//! Build script: records how this crate was built -- its target, and the wgpu it was built with -- for the runtime a
//! measurement's row names.

fn main() -> std::io::Result<()> {
    built::write_built_file()
}
