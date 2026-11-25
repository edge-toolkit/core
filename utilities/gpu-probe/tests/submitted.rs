//! Covers reading a plain-text file of submissions, as the hosted probe page proposes one on GitHub.
#![cfg(test)]

use et_gpu_probe_server::registry::{RegisterError, Submission, row};
use et_gpu_probe_server::submitted::{name_of, parse, propose, propose_where_asked, write};
use pretty_assertions::{assert_eq, assert_ne};

/// Two GPUs: one with a canvas signature and a withheld interface, the other with a wrapped shader signature.
const TWO_GPUS: &str = "label Canvas on Firefox
browser firefox
os linux
defence Z\\u{fc}rich \\\\ mode
window normal
canvas 4e31fa9b 76b62595 5debc5c0 52ef3664
webgl2 blocked: WebGL is disabled



label Apple GPU
browser chrome
webgpu-split 33800000 33800000 40800000 3eaaaaab 00000000 3fddb3d7 3f13cd3a 3dcc7576 3f7eb899 3dcd7c43
  3f8d763e 3f8c9f54 3f8edd1f
  3d087b99 3dcc1ebd 00000000";

/// The row the first block of [`TWO_GPUS`] gives, its escapes read.
const FIRST_ROW: [(&str, &str); 4] = [
    ("browser", "firefox"),
    ("defence", "Z\u{fc}rich \\ mode"),
    ("os", "linux"),
    ("window", "normal"),
];

/// Each of `pairs`, name beside value, as text to compare.
fn texts<'pairs, Pairs>(pairs: Pairs) -> Vec<(&'pairs str, &'pairs str)>
where
    Pairs: IntoIterator<Item = (&'pairs String, &'pairs String)>,
{
    pairs
        .into_iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// Each result of `submission`, name beside value.
fn results(submission: &Submission) -> Vec<(&str, &str)> {
    texts(submission.results.pairs().iter().map(|pair| (&pair.0, &pair.1)))
}

/// The fault `text` is refused for, its line and what is wrong.
fn fault(text: &str) -> (usize, &'static str) {
    match parse(text) {
        Err(RegisterError::Submitted { line, problem }) => (line, problem),
        other => panic!("{other:?}"),
    }
}

/// The submission at `index` of those [`TWO_GPUS`] holds, of which there are three.
fn two_gpus(index: usize) -> Submission {
    let mut submissions = parse(TWO_GPUS).unwrap();
    assert_eq!(submissions.len(), 3);
    submissions.swap_remove(index)
}

#[test]
fn a_signature_line_names_its_values_and_takes_the_label_and_row_of_its_block() {
    let canvas = two_gpus(0);
    assert_eq!(canvas.interface, "canvas");
    assert_eq!(canvas.label, "Canvas on Firefox");
    assert_eq!(texts(&canvas.seen), FIRST_ROW);
    assert!(!canvas.blocked);
    assert_eq!(
        results(&canvas),
        [
            ("lane0", "4e31fa9b"),
            ("lane1", "76b62595"),
            ("lane2", "5debc5c0"),
            ("lane3", "52ef3664"),
        ],
    );
}

#[test]
fn a_withheld_line_gives_its_reason_and_no_results() {
    let withheld = two_gpus(1);
    assert_eq!(withheld.interface, "webgl2");
    assert_eq!(withheld.label, "Canvas on Firefox");
    assert!(withheld.blocked);
    assert_eq!(withheld.reason, "WebGL is disabled");
    assert_eq!(texts(&withheld.seen), FIRST_ROW);
    assert_eq!(results(&withheld), []);
}

#[test]
fn a_wrapped_line_after_blank_lines_reads_as_one_line_of_the_next_block() {
    let split = two_gpus(2);
    assert_eq!(split.label, "Apple GPU");
    assert_eq!(texts(&split.seen), [("browser", "chrome")]);
    let values = results(&split);
    assert_eq!(values.len(), 16);
    assert_eq!(values[0], ("multiply_add", "33800000"));
    assert_eq!(values[11], ("log", "3f8c9f54"));
    assert_eq!(values[13], ("atan2", "3d087b99"));
    assert_eq!(values[15], ("dot", "00000000"));
}

#[test]
fn a_line_opening_three_spaces_continues_the_word_before_it() {
    let submissions = parse("label A\nos ab\n   cd\n  ef\n    gh\ncanvas 1 2 3 4\n").unwrap();
    assert_eq!(texts(&submissions[0].seen), [("os", "abcd ef gh")]);
}

#[test]
fn a_block_with_no_interface_line_is_refused() {
    assert_eq!(
        fault("label A\ncanvas 1 2 3 4\n\nlabel B\nbrowser firefox\n"),
        (4, "a block with no interface line"),
    );
}

#[test]
fn an_empty_file_holds_no_submissions() {
    assert_eq!(parse("").unwrap().len(), 0);
    assert_eq!(parse("\n\n").unwrap().len(), 0);
}

#[test]
fn a_line_out_of_place_is_named_by_its_number() {
    assert_eq!(fault("  continued\n"), (1, "a continuation with no line to continue"));
    assert_eq!(fault("   joined\n"), (1, "a continuation with no line to continue"));
    assert_eq!(
        fault("label A\n\n  continued\n"),
        (3, "a continuation with no line to continue"),
    );
    assert_eq!(
        fault("label A\ncanvas 1 2 3 4\nbrowser firefox\n"),
        (3, "a label or row field after the block's first interface"),
    );
    assert_eq!(
        fault("label A\ncanvas 1 2 3 4\nlabel B\n"),
        (3, "a label or row field after the block's first interface"),
    );
    assert_eq!(fault("label A\nlabel B\n"), (2, "a second label"));
    assert_eq!(fault("os linux\nos macos\n"), (2, "a row field given twice"));
    assert_eq!(
        fault("label A\ncanvas 1 2 3\n"),
        (2, "not as many values as the interface has results"),
    );
}

#[test]
fn an_escape_that_names_no_character_is_refused() {
    assert_eq!(fault("label \\u00e9\n"), (1, "a \\u escape with no braced code point"));
    assert_eq!(fault("label \\u{e9\n"), (1, "a \\u escape with no braced code point"));
    assert_eq!(fault("label \\u{d800}\n"), (1, "a \\u escape naming no character"));
    assert_eq!(fault("label \\u{xyz}\n"), (1, "a \\u escape naming no character"));
    assert_eq!(fault("label \\n\n"), (1, "a backslash that starts no escape"));
    assert_eq!(fault("label ends\\\n"), (1, "a backslash that starts no escape"));
    assert_eq!(fault("label A\nos \\q\n"), (2, "a backslash that starts no escape"));
    assert_eq!(
        fault("label A\nwebgl2 blocked: \\q\n"),
        (2, "a backslash that starts no escape"),
    );
}

#[test]
fn a_fault_reads_as_its_line_and_problem() {
    let refused = parse("label A\nlabel B\n").unwrap_err();
    assert_eq!(refused.to_string(), "line 2: a second label");
    assert!(!refused.is_rejection());
}

/// Submissions that try every part of the layout.
///
/// That is two blocks, escapes, a word too long for a line, double spaces, and a withheld interface.
fn awkward() -> Vec<Submission> {
    let long = format!("{} then {}  spaced", "x".repeat(150), "\u{e9}\\".repeat(30));
    let seen = row(&[
        ("browser", "none"),
        ("os", "linux"),
        ("runtime", "wgpu 29.0.4, built for x86_64-unknown-linux-gnu"),
        ("device", &long),
    ]);
    let lanes: Vec<(String, String)> = ["lane0", "lane1", "lane2", "lane3"]
        .iter()
        .zip(["00000001", "00000002", "00000003", "00000004"])
        .map(|(name, value)| ((*name).to_owned(), value.to_owned()))
        .collect();
    let shaders: Vec<(String, String)> = et_gpu_probe::result_names("webgpu-split")
        .into_iter()
        .map(|name| (name.to_owned(), "3f800000".to_owned()))
        .collect();
    vec![
        Submission::signature("canvas", &long, lanes, seen.clone()),
        Submission::signature("webgpu-split", "Second", shaders, row(&[("os", "macos")])),
        Submission::withheld("webgl2", &long, "no WebGL 2 \\ here", seen),
    ]
}

#[test]
fn what_is_written_reads_back_as_the_same_submissions_in_lines_a_tracked_file_allows() {
    let submissions = awkward();
    let text = write(&submissions);
    for line in text.lines() {
        assert!(line.len() <= 120);
        assert!(line.is_ascii());
        assert!(!line.ends_with(' '));
    }
    // The two blocks are separated by a blank line, the withheld interface joining the first block it shares.
    assert_eq!(text.matches("\n\n").count(), 1);
    let read = parse(&text).unwrap();
    // Each block holds every submission of its label and row, so the withheld one is read back beside the canvas.
    let grouped = [&submissions[0], &submissions[2], &submissions[1]];
    assert_eq!(read.len(), grouped.len());
    for (given, back) in grouped.into_iter().zip(&read) {
        assert_eq!(back.interface, given.interface);
        assert_eq!(back.label, given.label);
        assert_eq!(back.seen, given.seen);
        assert_eq!(back.blocked, given.blocked);
        assert_eq!(back.reason, given.reason);
        assert_eq!(back.results.pairs(), given.results.pairs());
    }
}

#[test]
fn a_file_is_named_for_its_text_with_a_hash_typos_passes_over() {
    let name = name_of("label A\n");
    assert_eq!(name, name_of("label A\n"));
    assert_ne!(name, name_of("label B\n"));
    let (stem, extension) = name.split_once('.').unwrap();
    assert_eq!(extension, "txt");
    assert_eq!(stem.len(), 32);
    assert!(stem.chars().all(|digit| digit.is_ascii_hexdigit()));
}

#[test]
fn a_proposal_is_written_where_asked_and_nowhere_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let into = dir.path().join("into");
    let submissions = awkward();
    let unasked = temp_env::with_var_unset("GPU_PROBE_INTO", || propose_where_asked(&submissions)).unwrap();
    assert_eq!(unasked, Vec::<std::path::PathBuf>::new());
    let into_text = into.to_str().unwrap();
    let asked = temp_env::with_var("GPU_PROBE_INTO", Some(into_text), || propose_where_asked(&submissions)).unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(parse(&fs_err::read_to_string(&asked[0]).unwrap()).unwrap().len(), 3);
    assert_eq!(asked[0], into.join(name_of(&write(&submissions))));
}

#[test]
fn a_proposal_into_a_path_that_is_a_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a file");
    fs_err::write(&file, "").unwrap();
    let refused = propose(&file, &awkward()).unwrap_err();
    assert!(
        refused
            .to_string()
            .starts_with("the file of submissions could not be written: ")
    );
}
