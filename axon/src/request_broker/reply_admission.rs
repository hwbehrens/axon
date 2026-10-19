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

use crate::ipc::{DaemonReply, encode_reply_line};
use crate::message::{Envelope, MAX_MESSAGE_SIZE};

/// True when `response` survives the wire bound and the requester-side
/// `DaemonReply::SendOk` framing.
///
/// `msg_id` is the request UUID the requester's `send` command echoes.
/// `Envelope::response_to` sets the same `from`/`to` the requester's daemon
/// restores on receipt and the serialization shape is identical, so the
/// wrapped size computed here matches the frame the requester will build.
/// The optional `req_id` echo is bounded at IPC ingress; a caller that
/// combines a near-limit response with a long `req_id` still receives the
/// spec'd explicit `message_too_large` instead of a truncated line.
pub(super) fn admits(msg_id: Uuid, response: &Envelope) -> bool {
    let wire_fits = response
        .wire_encode()
        .is_ok_and(|bytes| bytes.len() <= MAX_MESSAGE_SIZE as usize);
    if !wire_fits {
        return false;
    }
    let send_reply = DaemonReply::SendOk {
        ok: true,
        msg_id,
        req_id: None,
        response: Some(response.clone()),
    };
    encode_reply_line(&send_reply).is_ok()
}
