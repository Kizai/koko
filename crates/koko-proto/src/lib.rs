//! Shared protocol types and permission definitions for Koko.

pub const ALPN: &[u8] = b"koko/1";
pub const PAIR_ALPN: &[u8] = b"koko/pair/1";

// The first byte is the Control channel, then a big-endian u32 and its JSON payload.
pub const CONTROL_HELLO: &[u8] = b"\x01\x00\x00\x00\x07\"hello\"";
pub const CONTROL_READY: &[u8] = b"\x01\x00\x00\x00\x07\"ready\"";
