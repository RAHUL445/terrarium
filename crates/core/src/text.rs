//! The `text` merge rule: a Loro text CRDT per cell. A local write diffs
//! the old and new text into an update; applying updates in any order, any
//! number of times, gives the same text on every device.

use loro::{ExportMode, LoroDoc, UpdateOptions};

use crate::error::{Error, Result};
use crate::hlc::DeviceId;

const CONTAINER: &str = "text";

fn corrupt(e: impl std::fmt::Display) -> Error {
    Error::Corrupt(format!("text CRDT: {e}"))
}

/// Stable 64-bit Loro peer id for a device (FNV-1a of its id; never 0).
fn peer_id(device: &DeviceId) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device.0.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash.max(1)
}

fn load(state: Option<&[u8]>) -> Result<LoroDoc> {
    let doc = LoroDoc::new();
    if let Some(state) = state.filter(|s| !s.is_empty()) {
        doc.import(state).map_err(corrupt)?;
    }
    Ok(doc)
}

/// The update that turns the cell's text into `new`, or `None` if equal.
pub(crate) fn edit(state: Option<&[u8]>, device: &DeviceId, new: &str) -> Result<Option<Vec<u8>>> {
    let doc = load(state)?;
    doc.set_peer_id(peer_id(device)).map_err(corrupt)?;
    let text = doc.get_text(CONTAINER);
    if text.to_string() == new {
        return Ok(None);
    }
    let before = doc.oplog_vv();
    text.update(new, UpdateOptions::default())
        .map_err(corrupt)?;
    doc.commit();
    Ok(Some(
        doc.export(ExportMode::updates(&before)).map_err(corrupt)?,
    ))
}

/// Merges `update` into the cell; returns the new CRDT state and plain text.
pub(crate) fn apply(state: Option<&[u8]>, update: &[u8]) -> Result<(Vec<u8>, String)> {
    let doc = load(state)?;
    doc.import(update).map_err(corrupt)?;
    let text = doc.get_text(CONTAINER).to_string();
    Ok((doc.export(ExportMode::Snapshot).map_err(corrupt)?, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_edits_merge_in_any_order() {
        let a: DeviceId = "a".into();
        let b: DeviceId = "b".into();
        let base_update = edit(None, &a, "Buy milk").unwrap().unwrap();
        let (base, _) = apply(None, &base_update).unwrap();

        let from_a = edit(Some(&base), &a, "Buy oat milk").unwrap().unwrap();
        let from_b = edit(Some(&base), &b, "Buy milk and eggs").unwrap().unwrap();

        let (one, _) = apply(Some(&base), &from_a).unwrap();
        let (one, text_one) = apply(Some(&one), &from_b).unwrap();
        let (two, _) = apply(Some(&base), &from_b).unwrap();
        let (two, text_two) = apply(Some(&two), &from_a).unwrap();
        assert_eq!(text_one, text_two);
        assert_eq!(text_one, "Buy oat milk and eggs");

        // Re-applying is harmless.
        let (_, again) = apply(Some(&one), &from_a).unwrap();
        assert_eq!(again, text_one);
        assert_eq!(apply(Some(&two), &from_b).unwrap().1, text_two);
    }

    #[test]
    fn unchanged_text_makes_no_update() {
        let a: DeviceId = "a".into();
        let (state, _) = apply(None, &edit(None, &a, "same").unwrap().unwrap()).unwrap();
        assert!(edit(Some(&state), &a, "same").unwrap().is_none());
    }
}
