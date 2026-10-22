//! Recovery command fixture: emit both streams, then complete or wait for termination.

use std::io::{self, Write as _};

use clap::Parser as _;
use et_test_recovery::command::Arguments;

/// Keep the delayed mode finite so an interrupted parent cannot leave an indefinitely running fixture.
fn main() -> io::Result<()> {
    let arguments = Arguments::parse();
    let delay = arguments.delay();
    io::stdout().write_all(b"fixture stdout\n")?;
    io::stderr().write_all(b"fixture stderr\n")?;
    io::stdout().flush()?;
    io::stderr().flush()?;
    std::thread::sleep(delay);
    Ok(())
}
