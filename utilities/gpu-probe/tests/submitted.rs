//! Covers reading a plain-text file of submissions, as the hosted probe page proposes one on GitHub.
#![cfg(test)]

use et_gpu_probe_server::registry::{RegisterError, Submission};
use et_gpu_probe_server::submitted::parse;
use pretty_assertions::assert_eq;

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
            ("lane3", "52ef3664")
        ]
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
        (4, "a block with no interface line")
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
        (3, "a continuation with no line to continue")
    );
    assert_eq!(
        fault("label A\ncanvas 1 2 3 4\nbrowser firefox\n"),
        (3, "a label or row field after the block's first interface")
    );
    assert_eq!(
        fault("label A\ncanvas 1 2 3 4\nlabel B\n"),
        (3, "a label or row field after the block's first interface")
    );
    assert_eq!(fault("label A\nlabel B\n"), (2, "a second label"));
    assert_eq!(fault("os linux\nos macos\n"), (2, "a row field given twice"));
    assert_eq!(
        fault("label A\ncanvas 1 2 3\n"),
        (2, "not as many values as the interface has results")
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
        (2, "a backslash that starts no escape")
    );
}

#[test]
fn a_fault_reads_as_its_line_and_problem() {
    let refused = parse("label A\nlabel B\n").unwrap_err();
    assert_eq!(refused.to_string(), "line 2: a second label");
    assert!(!refused.is_rejection());
}
