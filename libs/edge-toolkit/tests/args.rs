#![cfg(test)]

#[rstest::rstest]
#[case::normal(vec!["et-ws-server"])]
#[case::unix_path(vec!["/path/to/et-ws-server"])]
#[case::windows_exe(vec!["et-ws-server.exe"])]
fn executable_name(#[case] args: Vec<&str>) {
    let args: Vec<String> = args.into_iter().map(String::from).collect();
    assert_eq!(
        edge_toolkit::args::executable_name_inner(args),
        "et-ws-server".to_string()
    );
}

#[cfg(windows)]
#[rstest::rstest]
#[case::windows_path(vec!["C:\\path\\to\\et-ws-server"])]
fn executable_name_windows(#[case] args: Vec<&str>) {
    executable_name(args);
}
