//! Admission of handler replies against every framing on their path.
//!
//! A reply must survive BOTH framings that carry it to the remote requester:
//! the QUIC wire frame (`message::MAX_MESSAGE_SIZE`, spec/WIRE_FORMAT.md
//! §5.2) and the requester daemon's IPC line that wraps the response in its
//! `send` reply (spec/IPC.md §2, §4.1). The wire bound alone is not enough:
//! the envelope gains `from`/`to` routing fields for IPC and the `send` reply
//! adds a JSON wrapper, so a wire-legal reply can still be refused when the
//! requester's daemon frames it — after the handler was already acked.
//! Modeling the exact wrapper here closes that gap.

use uuid::Uuid;

use crate::ipc::{DaemonReply, MAX_REQ_ID_BYTES, encode_reply_line};
use crate::message::{Envelope, MAX_MESSAGE_SIZE};

/// True when `response` survives the wire bound and the requester-side
/// `DaemonReply::SendOk` framing.
///
/// `msg_id` is the request UUID the requester's `send` command echoes.
/// `Envelope::response_to` sets the same `from`/`to` the requester's daemon
/// restores on receipt and the serialization shape is identical, so the
/// wrapped size computed here matches the frame the requester will build.
///
/// The requester's `req_id` is NOT carried on the wire, so the model
/// reserves its worst case: `MAX_REQ_ID_BYTES` raw bytes, each escaped by
/// serde_json into a 6-byte `\uXXXX` sequence. Modeling only the ingress
/// bound let a near-limit response be admitted and acked here, then replaced
/// by the requester's `message_too_large` fallback — completing the request
/// without delivering the response.
pub(super) fn admits(msg_id: Uuid, response: &Envelope) -> bool {
    let wire_fits = response
        .wire_encode()
        .is_ok_and(|bytes| bytes.len() <= MAX_MESSAGE_SIZE as usize);
    if !wire_fits {
        return false;
    }
    // NUL escapes to `\u0000` (six bytes), the maximal JSON expansion per
    // input byte, so this models the largest possible echoed `req_id`.
    let worst_case_req_id = "\u{0}".repeat(MAX_REQ_ID_BYTES);
    let send_reply = DaemonReply::SendOk {
        ok: true,
        msg_id,
        req_id: Some(worst_case_req_id),
        response: Some(response.clone()),
    };
    encode_reply_line(&send_reply).is_ok()
}
