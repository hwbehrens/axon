use super::*;
use serde_json::json;

#[test]
fn hello_payload_serde() {
    let p = HelloPayload {
        protocol_versions: vec![1],
        selected_version: None,
        agent_name: Some("Test".to_string()),
        features: vec!["delegate".to_string()],
    };
    let v = serde_json::to_value(&p).unwrap();
    let back: HelloPayload = serde_json::from_value(v).unwrap();
    assert_eq!(p, back);
}

#[test]
fn hello_response_payload_serde() {
    let p = HelloPayload {
        protocol_versions: vec![1],
        selected_version: Some(1),
        agent_name: None,
        features: vec![],
    };
    let v = serde_json::to_value(&p).unwrap();
    let back: HelloPayload = serde_json::from_value(v).unwrap();
    assert_eq!(p, back);
}

#[test]
fn ping_pong_payload_serde() {
    let ping = PingPayload {};
    let v = serde_json::to_value(&ping).unwrap();
    let _: PingPayload = serde_json::from_value(v).unwrap();

    let pong = PongPayload {
        status: PeerStatus::Idle,
        uptime_secs: 3600,
        active_tasks: 2,
        agent_name: Some("Agent".to_string()),
    };
    let v = serde_json::to_value(&pong).unwrap();
    let back: PongPayload = serde_json::from_value(v).unwrap();
    assert_eq!(pong, back);
}

#[test]
fn query_response_payload_serde() {
    let q = QueryPayload {
        question: "test?".to_string(),
        domain: Some("meta.status".to_string()),
        max_tokens: Some(200),
        deadline_ms: Some(30000),
    };
    let v = serde_json::to_value(&q).unwrap();
    let back: QueryPayload = serde_json::from_value(v).unwrap();
    assert_eq!(q, back);

    let r = ResponsePayload {
        data: json!({"events": []}),
        summary: "None".to_string(),
        tokens_used: Some(5),
        truncated: Some(false),
    };
    let v = serde_json::to_value(&r).unwrap();
    let back: ResponsePayload = serde_json::from_value(v).unwrap();
    assert_eq!(r, back);
}

#[test]
fn delegate_payload_defaults() {
    let json = json!({"task": "do something"});
    let d: DelegatePayload = serde_json::from_value(json).unwrap();
    assert_eq!(d.priority, Priority::Normal);
    assert!(d.report_back);
    assert!(d.context.is_none());
    assert!(d.deadline_ms.is_none());
}

#[test]
fn ack_result_payload_serde() {
    let ack = AckPayload {
        accepted: true,
        estimated_ms: Some(5000),
    };
    let v = serde_json::to_value(&ack).unwrap();
    let back: AckPayload = serde_json::from_value(v).unwrap();
    assert_eq!(ack, back);

    let res = ResultPayload {
        status: TaskStatus::Completed,
        outcome: "Done".to_string(),
        data: None,
        error: None,
    };
    let v = serde_json::to_value(&res).unwrap();
    let back: ResultPayload = serde_json::from_value(v).unwrap();
    assert_eq!(res, back);
}

#[test]
fn result_failed_payload_serde() {
    let res = ResultPayload {
        status: TaskStatus::Failed,
        outcome: "Could not send".to_string(),
        data: None,
        error: Some("Service unavailable".to_string()),
    };
    let v = serde_json::to_value(&res).unwrap();
    let back: ResultPayload = serde_json::from_value(v).unwrap();
    assert_eq!(res, back);
}

#[test]
fn notify_payload_defaults() {
    let json = json!({"topic": "test", "data": {}});
    let n: NotifyPayload = serde_json::from_value(json).unwrap();
    assert_eq!(n.importance, Importance::Low);
}

#[test]
fn cancel_payload_serde() {
    let c = CancelPayload {
        reason: Some("Plans changed".to_string()),
    };
    let v = serde_json::to_value(&c).unwrap();
    let back: CancelPayload = serde_json::from_value(v).unwrap();
    assert_eq!(c, back);
}

#[test]
fn cancel_missing_reason_tolerant() {
    let result = serde_json::from_value::<CancelPayload>(json!({}));
    assert!(
        result.is_ok(),
        "must tolerate missing reason for backward compat"
    );
    assert_eq!(result.unwrap().reason, None);
}

#[test]
fn result_error_omitted_when_none() {
    let res = ResultPayload {
        status: TaskStatus::Completed,
        outcome: "Done".to_string(),
        data: None,
        error: None,
    };
    let v = serde_json::to_value(&res).unwrap();
    assert!(
        v.get("error").is_none(),
        "error field must be omitted when None"
    );
}

#[test]
fn discover_capabilities_payload_serde() {
    let d = DiscoverPayload {};
    let v = serde_json::to_value(&d).unwrap();
    let _: DiscoverPayload = serde_json::from_value(v).unwrap();

    let caps = CapabilitiesPayload {
        agent_name: Some("Family Assistant".to_string()),
        domains: vec!["family".to_string()],
        channels: vec!["imessage".to_string()],
        tools: vec!["web_search".to_string()],
        max_concurrent_tasks: Some(4),
        model: Some("gemini-3-pro".to_string()),
    };
    let v = serde_json::to_value(&caps).unwrap();
    let back: CapabilitiesPayload = serde_json::from_value(v).unwrap();
    assert_eq!(caps, back);
}

#[test]
fn error_payload_serde() {
    let e = ErrorPayload {
        code: ErrorCode::UnknownDomain,
        message: "I don't handle that".to_string(),
        retryable: false,
    };
    let v = serde_json::to_value(&e).unwrap();
    let back: ErrorPayload = serde_json::from_value(v).unwrap();
    assert_eq!(e, back);
}

#[test]
fn peer_status_snake_case() {
    assert_eq!(
        serde_json::to_string(&PeerStatus::Idle).unwrap(),
        "\"idle\""
    );
    assert_eq!(
        serde_json::to_string(&PeerStatus::Busy).unwrap(),
        "\"busy\""
    );
    assert_eq!(
        serde_json::to_string(&PeerStatus::Overloaded).unwrap(),
        "\"overloaded\""
    );
}

#[test]
fn priority_snake_case() {
    assert_eq!(
        serde_json::to_string(&Priority::Normal).unwrap(),
        "\"normal\""
    );
    assert_eq!(
        serde_json::to_string(&Priority::Urgent).unwrap(),
        "\"urgent\""
    );
}

#[test]
fn task_status_snake_case() {
    assert_eq!(
        serde_json::to_string(&TaskStatus::Completed).unwrap(),
        "\"completed\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Failed).unwrap(),
        "\"failed\""
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Partial).unwrap(),
        "\"partial\""
    );
}

#[test]
fn importance_snake_case() {
    assert_eq!(serde_json::to_string(&Importance::Low).unwrap(), "\"low\"");
    assert_eq!(
        serde_json::to_string(&Importance::Medium).unwrap(),
        "\"medium\""
    );
    assert_eq!(
        serde_json::to_string(&Importance::High).unwrap(),
        "\"high\""
    );
}

#[test]
fn error_code_snake_case() {
    assert_eq!(
        serde_json::to_string(&ErrorCode::NotAuthorized).unwrap(),
        "\"not_authorized\""
    );
    assert_eq!(
        serde_json::to_string(&ErrorCode::IncompatibleVersion).unwrap(),
        "\"incompatible_version\""
    );
    assert_eq!(
        serde_json::to_string(&ErrorCode::PeerNotFound).unwrap(),
        "\"peer_not_found\""
    );
}
