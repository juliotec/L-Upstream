//! Little-endian fields of the game's files, read without trusting their place: a field past the bytes is `None` (or
//! 0 for the `le` forms, where the caller checked the length before).

/// The `u32` at `at`, if the bytes hold it.
pub(crate) fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The `u64` at `at`, if the bytes hold it.
pub(crate) fn u64_at(b: &[u8], at: u64) -> Option<u64> {
    let at = usize::try_from(at).ok()?;
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

/// The `i32` at `at`, widened, if the bytes hold it.
pub(crate) fn i32_at(b: &[u8], at: u64) -> Option<i64> {
    let at = usize::try_from(at).ok()?;
    Some(i64::from(i32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?)))
}

/// The `u32` at `at` (0 past the bytes).
pub(crate) fn le32(b: &[u8], at: usize) -> u32 {
    u32_at(b, at).unwrap_or(0)
}

/// The `u64` at `at` (0 past the bytes).
pub(crate) fn le64(b: &[u8], at: usize) -> u64 {
    u64_at(b, at as u64).unwrap_or(0)
}
