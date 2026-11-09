//! Covers the simulated controller board's frames against one whose CRC Hiwonder's own lookup table gives.
#![cfg(test)]

use et_ws_test_server::imu::{FUNCTION_IMU, frame};

#[test]
fn a_frame_carries_the_crc_hiwonders_table_gives() {
    let data: Vec<u8> = (0..24).collect();
    let bytes = frame(FUNCTION_IMU, &data);
    assert_eq!(bytes[..4], [0xAA, 0x55, FUNCTION_IMU, 24]);
    assert_eq!(bytes[4..28], data[..]);
    assert_eq!(bytes[28..], [0x94]);
}
