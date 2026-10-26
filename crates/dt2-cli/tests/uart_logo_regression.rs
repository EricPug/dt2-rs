use std::fs;
use std::path::PathBuf;

const EXPECTED_LENGTH: usize = 1321;
const EXPECTED_FNV1A64: u64 = 0x74b7_474d_6f78_3291;

#[test]
#[ignore = "requires the local, ignored firmware capture at out/pit0-natural-run.log"]
fn natural_pit0_uart_stream_matches_os_117_capture() {
    let capture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../out/pit0-natural-run.log");
    let capture = fs::read_to_string(&capture_path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", capture_path.display()));
    let hex = capture
        .lines()
        .find_map(|line| line.strip_prefix("UART8 TX hex: "))
        .expect("capture log contains a UART8 TX hex line");
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).expect("valid UART hex byte"))
        .collect();

    assert_eq!(bytes.len(), EXPECTED_LENGTH);
    assert_eq!(fnv1a64(&bytes), EXPECTED_FNV1A64);
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}
