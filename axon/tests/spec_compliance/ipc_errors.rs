//! IPC error-code contract between the implementation and `spec/IPC.md`.
//!
//! Guards the review finding that a new runtime error code
//! (`send_capacity_exceeded`) was emitted by the daemon while missing from
//! the normative spec table: any `IpcErrorCode` variant must be documented
//! in `spec/IPC.md`'s error table, and the wire spelling must match the
//! serde rendering clients actually receive.

use axon::ipc::IpcErrorCode;

/// Every error code the daemon can emit on the wire.
const ALL_CODES: &[IpcErrorCode] = &[
    IpcErrorCode::InvalidCommand,
    IpcErrorCode::CommandTooLarge,
    IpcErrorCode::PeerNotFound,
    IpcErrorCode::PeerNotObserved,
    IpcErrorCode::PeerConflict,
    IpcErrorCode::SelfSend,
    IpcErrorCode::PeerUnreachable,
    IpcErrorCode::Timeout,
    IpcErrorCode::HandlerBusy,
    IpcErrorCode::NotHandler,
    IpcErrorCode::RequestNotFound,
    IpcErrorCode::SendCapacityExceeded,
    IpcErrorCode::InternalError,
];

#[test]
fn every_ipc_error_code_is_documented_in_the_spec_table() {
    let spec = include_str!("../../../spec/IPC.md");
    for code in ALL_CODES {
        let rendered = serde_json::to_string(code).expect("error codes serialize");
        let token = rendered.trim_matches('"');
        assert!(
            spec.contains(&format!("`{token}`")),
            "error code `{token}` is emitted by the daemon but missing from \
             spec/IPC.md's error table; update the spec in the same change"
        );
    }
}

#[test]
fn error_code_wire_spelling_is_snake_case() {
    // The spec table documents snake_case spellings; this pins the serde
    // rendering so a rename cannot silently desynchronize the contract.
    let rendered = serde_json::to_string(&IpcErrorCode::SendCapacityExceeded).unwrap();
    assert_eq!(rendered, "\"send_capacity_exceeded\"");
}
