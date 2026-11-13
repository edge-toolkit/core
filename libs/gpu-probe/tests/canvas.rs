//! Covers the canvas fingerprints' pixel hash, against vectors the hash the registry's canvas entries were taken with
//! gave for the same bytes.
#![cfg(test)]

use et_gpu_probe::{
    CANVAS, CANVAS_BROWSERLEAKS, KNOWN_BLOCK, KNOWN_SIZE, alteration, known_colour, known_scene, pixel_hash,
};

#[test]
fn known_colours_wrap_at_a_byte() {
    assert_eq!(known_colour(0, 0), [20, 70, 130]);
    assert_eq!(known_colour(1, 0), [57, 129, 147]);
    assert_eq!(known_colour(7, 7), [100, 132, 188]);
}

#[test]
fn the_known_scene_is_opaque_blocks() {
    let scene = known_scene();
    let side = usize::from(KNOWN_SIZE);
    assert_eq!(scene.len(), side * side * 4);
    let pixel = |column: usize, row: usize| scene[(row * side + column) * 4..][..4].to_vec();
    let block = usize::from(KNOWN_BLOCK);
    assert_eq!(pixel(0, 0), [20, 70, 130, 255]);
    assert_eq!(pixel(block - 1, block - 1), [20, 70, 130, 255]);
    assert_eq!(pixel(block, 0), [57, 129, 147, 255]);
    assert_eq!(pixel(side - 1, side - 1), [100, 132, 188, 255]);
}

#[test]
fn a_faithful_read_is_unaltered() {
    let scene = known_scene();
    let unaltered = alteration(&scene, &scene).unwrap();
    assert_eq!(
        (unaltered.changed, unaltered.total, unaltered.pattern.as_str()),
        (0, scene.len(), "")
    );
}

#[test]
fn noise_is_counted_and_told_apart() {
    let scene = known_scene();
    let mut once = scene.clone();
    once[5] ^= 1;
    let mut twice = once.clone();
    twice[900] ^= 2;
    let first = alteration(&once, &scene).unwrap();
    let second = alteration(&twice, &scene).unwrap();
    assert_eq!((first.changed, second.changed), (1, 2));
    assert_eq!(first.pattern.len(), 35);
    assert_ne!(first.pattern, second.pattern);
}

#[test]
fn a_read_of_the_wrong_length_is_refused() {
    let refused = alteration(&[0; 3], &[0; 4]).unwrap_err();
    assert_eq!((refused.read, refused.expected), (3, 4));
    assert_eq!(refused.to_string(), "read 3 values, not 4");
}

/// `length` bytes counting up in sevens from three, wrapping at a byte, so every lane sees every byte value.
fn sequence(length: usize) -> Vec<u8> {
    core::iter::successors(Some(3_u8), |byte| Some(byte.wrapping_add(7)))
        .take(length)
        .collect()
}

#[test]
fn nothing_hashes_to_the_seeds() {
    assert_eq!(pixel_hash(&[]), [0x811c_9dc5, 0x1b87_3593, 0x9e37_79b1, 0x85eb_ca77]);
}

#[test]
fn one_block_folds_once() {
    assert_eq!(
        pixel_hash(&sequence(64)),
        [0x71b1_e1c5, 0x09e0_6c13, 0xb3d5_03be, 0x9a5a_42af]
    );
}

#[test]
fn a_partial_block_after_several_whole_ones_is_taken_in() {
    assert_eq!(
        pixel_hash(&sequence(300)),
        [0xe5e7_e389, 0x4e56_3baf, 0x7e05_84bb, 0x34e9_227b]
    );
}

#[test]
fn one_changed_byte_changes_every_lane() {
    let mut bytes = sequence(300);
    let before = pixel_hash(&bytes);
    bytes[150] ^= 1;
    let after = pixel_hash(&bytes);
    assert!(before.iter().zip(after).all(|(was, now)| *was != now));
}

#[test]
fn the_interfaces_are_named_as_the_registry_files_them() {
    assert_eq!(CANVAS, "canvas");
    assert_eq!(CANVAS_BROWSERLEAKS, "canvas-browserleaks");
}
