//! Build script: records how this binary was built, for the `/info` route to report.

fn main() -> std::io::Result<()> {
    built::write_built_file()
}
