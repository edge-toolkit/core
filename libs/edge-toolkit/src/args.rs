/// Return the executable name that was invoked.
///
/// Removes `.exe` suffix.
///
/// # Panics
/// This function will panic if there is no command line argument 0
/// which may happen if the invoking environment is not similar to a "std" environment.
#[must_use]
pub fn executable_name() -> String {
    executable_name_inner(std::env::args().collect())
}

#[expect(clippy::unwrap_used)]
pub fn executable_name_inner(args: Vec<String>) -> String {
    let path = args.first().unwrap();
    let path = std::path::PathBuf::from(path);
    path.file_stem().unwrap().to_string_lossy().to_string()
}
