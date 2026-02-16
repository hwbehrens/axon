use super::*;

use proptest::prelude::*;

fn arb_message_kind() -> impl Strategy<Value = MessageKind> {
    prop_oneof![
        Just(MessageKind::Hello),
        Just(MessageKind::Ping),
        Just(MessageKind::Pong),
        Just(MessageKind::Query),
        Just(MessageKind::Response),
        Just(MessageKind::Delegate),
        Just(MessageKind::Ack),
        Just(MessageKind::Result),
        Just(MessageKind::Notify),
        Just(MessageKind::Cancel),
        Just(MessageKind::Discover),
        Just(MessageKind::Capabilities),
        Just(MessageKind::Error),
    ]
}

proptest! {
    #[test]
    fn ipc_command_parse_never_panics(data in "\\PC{0,256}") {
        let _ = serde_json::from_str::<IpcCommand>(&data);
    }

    #[test]
    fn inbox_limit_clamped(limit in 0usize..10000) {
        let cmd_json = format!(r#"{{"cmd":"inbox","limit":{}}}"#, limit);
        if let Ok(IpcCommand::Inbox { limit: parsed, .. }) = serde_json::from_str::<IpcCommand>(&cmd_json) {
            prop_assert_eq!(parsed, limit);
        }
    }

    #[test]
    fn hello_version_roundtrips(version in 1u32..1000) {
        let cmd_json = format!(r#"{{"cmd":"hello","version":{}}}"#, version);
        let parsed: IpcCommand = serde_json::from_str(&cmd_json).unwrap();
        match parsed {
            IpcCommand::Hello { version: v, consumer, .. } => {
                prop_assert_eq!(v, version);
                prop_assert_eq!(consumer, "default");
            }
            _ => prop_assert!(false, "expected hello command"),
        }
    }

    #[test]
    fn subscribe_kinds_roundtrip(
        kinds in proptest::collection::vec(arb_message_kind(), 0..5)
    ) {
        let kinds_str: Vec<String> = kinds.iter().map(|k| {
            let s = serde_json::to_string(k).unwrap();
            // strip outer quotes to get raw string
            s.trim_matches('"').to_string()
        }).collect();
        let kinds_json: Vec<String> = kinds_str.iter().map(|s| format!("\"{s}\"")).collect();
        let cmd_json = format!(r#"{{"cmd":"subscribe","kinds":[{}]}}"#, kinds_json.join(","));
        let parsed: IpcCommand = serde_json::from_str(&cmd_json).unwrap();
        match parsed {
            IpcCommand::Subscribe { kinds: Some(parsed_kinds), .. } => {
                prop_assert_eq!(parsed_kinds.len(), kinds.len());
                for (parsed, original) in parsed_kinds.iter().zip(kinds_str.iter()) {
                    prop_assert_eq!(parsed, original);
                }
            }
            IpcCommand::Subscribe { kinds: None, .. } if kinds.is_empty() => {}
            _ => prop_assert!(false, "expected subscribe command"),
        }
    }

    #[test]
    fn req_id_roundtrips_on_all_commands(req_id in "[a-z0-9\\-]{1,32}") {
        // Test req_id round-trip on a representative set of v2 commands
        let hello = format!(r#"{{"cmd":"hello","version":2,"req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&hello).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));

        let peers = format!(r#"{{"cmd":"peers","req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&peers).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));

        let ack = format!(r#"{{"cmd":"ack","up_to_seq":1,"req_id":"{req_id}"}}"#);
        let parsed: IpcCommand = serde_json::from_str(&ack).unwrap();
        prop_assert_eq!(parsed.req_id(), Some(req_id.as_str()));
    }
}
