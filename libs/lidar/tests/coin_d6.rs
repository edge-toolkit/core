//! Covers the COIN-D6 encoder and decoder: framing, checksums, angles, resync, revolutions and what cannot encode.
#![cfg(test)]

use et_lidar::coin_d6::{Decoder, FULL_TURN, MAX_DISTANCE_MM, Packet, Sample, Scan, Scanner};

/// A sample `distance_mm` away at `angle`, its echo of middling strength.
fn sample_at(angle: u16, distance_mm: u16) -> Sample {
    Sample::new(angle, distance_mm, 100, false)
}

/// The bytes of a packet that starts a revolution at 10 Hz, its one sample `distance_mm` away.
fn ring_start(distance_mm: u16) -> Vec<u8> {
    Packet::new(true, 100, vec![sample_at(0, distance_mm)])
        .encode()
        .unwrap()
}

/// The bytes of a packet within a revolution, its samples `distances` away and spread from `first` to `last`.
fn within(first: u16, last: u16, distances: &[u16]) -> Vec<u8> {
    let samples = distances
        .iter()
        .enumerate()
        .map(|(index, &distance)| sample_at(if index == 0 { first } else { last }, distance))
        .collect();
    Packet::new(false, 0, samples).encode().unwrap()
}

/// The one packet `bytes` decode to.
fn only_packet(bytes: &[u8]) -> Packet {
    let mut packets = Decoder::default().push(bytes);
    assert_eq!(packets.len(), 1);
    packets.remove(0)
}

/// Each sample's angle.
fn angles(packet: &Packet) -> Vec<u16> {
    packet.samples.iter().map(|sample| sample.angle).collect()
}

/// Each sample's distance.
fn distances(scan: &Scan) -> Vec<u16> {
    scan.samples.iter().map(|sample| sample.distance_mm).collect()
}

#[test]
fn a_packet_encoded_by_hand_decodes_and_encodes_back() {
    // A revolution's first packet at 10.0 Hz: one sample at 0 degrees, 1000 mm away at intensity 200.
    let bytes = [
        0xAA, 0x55, 0xC9, 0x01, 0x01, 0x00, 0x01, 0x00, 0xE0, 0x5B, 0x20, 0xA3, 0x0F,
    ];
    let packet = only_packet(&bytes);
    assert!(packet.starts_revolution);
    assert_eq!(packet.frequency_dhz, 100);
    let [sample] = packet.samples[..] else {
        panic!("one sample expected, got {:?}", packet.samples);
    };
    assert_eq!(sample.angle, 0);
    assert_eq!(sample.distance_mm, 1000);
    assert_eq!(sample.intensity, 200);
    assert!(!sample.high_reflectivity);
    assert_eq!(packet.encode().unwrap(), bytes);
}

#[test]
fn samples_are_spread_evenly_from_the_first_angle_to_the_last() {
    let packet = only_packet(&within(10 * 64, 12 * 64, &[500, 600, 700]));
    assert!(!packet.starts_revolution);
    assert_eq!(angles(&packet), [640, 704, 768]);
    let degrees: Vec<f64> = packet.samples.iter().map(|sample| sample.degrees()).collect();
    assert_eq!(degrees, [10.0_f64, 11.0_f64, 12.0_f64]);
    let distances: Vec<u16> = packet.samples.iter().map(|sample| sample.distance_mm).collect();
    assert_eq!(distances, [500, 600, 700]);
}

#[test]
fn angles_wrap_past_a_full_turn() {
    let packet = only_packet(&within(FULL_TURN - 64, 64, &[1, 2, 3]));
    assert_eq!(angles(&packet), [FULL_TURN - 64, 0, 64]);
}

#[test]
fn the_high_reflectivity_flag_and_the_widest_values_decode() {
    let bytes = Packet::new(false, 0, vec![Sample::new(0, MAX_DISTANCE_MM, 255, true)])
        .encode()
        .unwrap();
    let packet = only_packet(&bytes);
    let [sample] = packet.samples[..] else {
        panic!("one sample expected, got {:?}", packet.samples);
    };
    assert_eq!(sample.distance_mm, MAX_DISTANCE_MM);
    assert_eq!(sample.intensity, 255);
    assert!(sample.high_reflectivity);
}

#[test]
fn a_packet_the_protocol_has_no_room_for_does_not_encode() {
    let cases = [
        (
            Packet::new(false, 0, Vec::new()),
            "a packet holds 1 to 255 samples, not 0",
        ),
        (
            Packet::new(false, 0, vec![sample_at(0, 1); 256]),
            "a packet holds 1 to 255 samples, not 256",
        ),
        (
            Packet::new(false, 0, vec![sample_at(FULL_TURN, 1)]),
            "an angle is below 23040, not 23040",
        ),
        (
            Packet::new(false, 0, vec![sample_at(0, 1), sample_at(FULL_TURN, 1)]),
            "an angle is below 23040, not 23040",
        ),
        (
            Packet::new(false, 0, vec![sample_at(0, 0x4000)]),
            "a distance is at most 16383 mm, not 16384",
        ),
        (
            Packet::new(true, 128, vec![sample_at(0, 1)]),
            "a scan frequency is at most 127 tenths of a hertz, not 128",
        ),
    ];
    for (packet, message) in cases {
        assert_eq!(packet.encode().unwrap_err().to_string(), message);
    }
}

#[test]
fn a_packet_arriving_a_byte_at_a_time_decodes_once_whole() {
    let bytes = within(0, 64, &[1, 2]);
    let mut decoder = Decoder::default();
    let mut counts = Vec::new();
    for byte in &bytes {
        counts.push(decoder.push(&[*byte]).len());
    }
    let (last, earlier) = counts.split_last().unwrap();
    assert_eq!(*last, 1);
    assert!(earlier.iter().all(|&count| count == 0));
    assert_eq!(decoder.skipped_bytes(), 0);
}

#[test]
fn bytes_before_a_header_are_skipped_and_counted() {
    let mut decoder = Decoder::default();
    assert!(decoder.push(&[0x01, 0x02, 0x03]).is_empty());
    assert_eq!(decoder.skipped_bytes(), 3);
    let mut bytes = vec![0x04, 0x05];
    bytes.extend(ring_start(10));
    assert_eq!(decoder.push(&bytes).len(), 1);
    assert_eq!(decoder.skipped_bytes(), 5);
    assert_eq!(decoder.rejected_packets(), 0);
}

#[test]
fn a_last_byte_that_may_start_a_header_waits_for_the_next() {
    let bytes = ring_start(10);
    let (first, rest) = bytes.split_first().unwrap();
    let mut decoder = Decoder::default();
    assert!(decoder.push(&[0x01, *first]).is_empty());
    assert_eq!(decoder.skipped_bytes(), 1);
    assert_eq!(decoder.push(rest).len(), 1);
    assert_eq!(decoder.skipped_bytes(), 1);
}

#[test]
fn a_bad_checksum_drops_the_packet_and_the_next_one_still_decodes() {
    let mut corrupt = within(0, 64, &[1, 2]);
    *corrupt.last_mut().unwrap() ^= 0x01;
    let mut bytes = corrupt.clone();
    bytes.extend(ring_start(10));
    let mut decoder = Decoder::default();
    let packets = decoder.push(&bytes);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].starts_revolution);
    assert_eq!(decoder.rejected_packets(), 1);
    assert_eq!(decoder.skipped_bytes(), u64::try_from(corrupt.len()).unwrap());
}

#[test]
fn a_header_with_no_samples_or_a_clear_check_bit_is_rejected() {
    let mut empty = within(0, 64, &[1, 2]);
    empty[3] = 0;
    let mut first_unchecked = within(0, 64, &[1, 2]);
    first_unchecked[4] &= !1;
    let mut last_unchecked = within(0, 64, &[1, 2]);
    last_unchecked[6] &= !1;
    for bytes in [empty, first_unchecked, last_unchecked] {
        let mut decoder = Decoder::default();
        assert!(decoder.push(&bytes).is_empty());
        assert_eq!(decoder.rejected_packets(), 1);
    }
}

#[test]
fn a_scanner_drops_the_partial_revolution_and_completes_each_when_the_next_starts() {
    let mut scanner = Scanner::default();
    let mut bytes = within(FULL_TURN - 128, FULL_TURN - 64, &[1, 2]);
    bytes.extend(ring_start(10));
    bytes.extend(within(64, 128, &[11, 12]));
    assert!(scanner.push(&bytes).is_empty());

    let scans = scanner.push(&ring_start(20));
    let [scan] = &*scans else {
        panic!("one scan expected, got {scans:?}");
    };
    assert_eq!(scan.frequency_dhz, 100);
    assert_eq!([scan.frequency_hz()], [10.0_f64]);
    assert_eq!(distances(scan), [10, 11, 12]);

    let scans = scanner.push(&ring_start(30));
    let [scan] = &*scans else {
        panic!("one scan expected, got {scans:?}");
    };
    assert_eq!(distances(scan), [20]);
    assert_eq!(scanner.decoder().rejected_packets(), 0);
    assert_eq!(scanner.decoder().skipped_bytes(), 0);
}
