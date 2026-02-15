use super::*;

// =========================================================================
// §Payload Schemas — round-trips matching spec examples
// =========================================================================

/// Spec example hello initiating payload.
#[test]
fn hello_initiating_matches_spec_shape() {
    let payload = HelloPayload {
        protocol_versions: vec![1],
        selected_version: None,
        agent_name: Some("TestAgent".to_string()),
        features: vec!["delegate".to_string(), "discover".to_string()],
    };
    let env = Envelope::new(
        agent_a(),
        agent_b(),
        MessageKind::Hello,
        serde_json::to_value(&payload).unwrap(),
    );
    let j = to_json(&env);
    assert_eq!(j["kind"], "hello");
    let p = &j["payload"];
    assert_eq!(p["protocol_versions"], json!([1]));
    assert_eq!(p["agent_name"], "TestAgent");
    assert_eq!(p["features"], json!(["delegate", "discover"]));
    // Initiating hello should NOT have selected_version
    assert!(
        p.get("selected_version").is_none() || p["selected_version"].is_null(),
        "initiating hello should not have selected_version"
    );
}

/// Spec example hello response payload.
#[test]
fn hello_response_matches_spec_shape() {
    let payload = HelloPayload {
        protocol_versions: vec![1],
        selected_version: Some(1),
        agent_name: None,
        features: vec![
            "delegate".to_string(),
            "discover".to_string(),
            "cancel".to_string(),
        ],
    };
    let req = Envelope::new(agent_a(), agent_b(), MessageKind::Hello, json!({}));
    let resp = Envelope::response_to(
        &req,
        agent_b(),
        MessageKind::Hello,
        serde_json::to_value(&payload).unwrap(),
    );
    let j = to_json(&resp);
    assert_eq!(j["kind"], "hello");
    assert_eq!(j["ref"].as_str().unwrap(), req.id.to_string());
    let p = &j["payload"];
    assert_eq!(p["selected_version"], 1);
    assert_eq!(p["features"], json!(["delegate", "discover", "cancel"]));
}

/// message-types.md §ping: empty payload.
#[test]
fn ping_payload_is_empty_object() {
    let p = PingPayload {};
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v, json!({}));
}

/// message-types.md §pong: status, uptime_secs, active_tasks, agent_name.
#[test]
fn pong_payload_matches_spec() {
    let p = PongPayload {
        status: PeerStatus::Idle,
        uptime_secs: 3600,
        active_tasks: 2,
        agent_name: Some("MyAgent".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["status"], "idle");
    assert_eq!(v["uptime_secs"], 3600);
    assert_eq!(v["active_tasks"], 2);
    assert_eq!(v["agent_name"], "MyAgent");
}

/// message-types.md §query: question, domain, max_tokens, deadline_ms.
#[test]
fn query_payload_matches_spec() {
    let p = QueryPayload {
        question: "What events are on the family calendar this week?".to_string(),
        domain: Some("family.calendar".to_string()),
        max_tokens: Some(200),
        deadline_ms: Some(30000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["question"],
        "What events are on the family calendar this week?"
    );
    assert_eq!(v["domain"], "family.calendar");
    assert_eq!(v["max_tokens"], 200);
    assert_eq!(v["deadline_ms"], 30000);
}

/// message-types.md §response: data, summary, tokens_used, truncated.
#[test]
fn response_payload_matches_spec() {
    let p = ResponsePayload {
        data: json!({"events": [{"name": "swim practice"}]}),
        summary: "Three swim practices this week: Mon/Wed/Fri 4-5pm".to_string(),
        tokens_used: Some(47),
        truncated: Some(false),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["summary"],
        "Three swim practices this week: Mon/Wed/Fri 4-5pm"
    );
    assert_eq!(v["tokens_used"], 47);
    assert_eq!(v["truncated"], false);
}

/// message-types.md §delegate: task, context, priority, report_back, deadline_ms.
#[test]
fn delegate_payload_matches_spec() {
    let p = DelegatePayload {
        task: "Send a message to the family group chat about dinner plans".to_string(),
        context: Some(json!({"dinner_time": "7:00 PM", "location": "home"})),
        priority: Priority::Normal,
        report_back: true,
        deadline_ms: Some(60000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["task"],
        "Send a message to the family group chat about dinner plans"
    );
    assert_eq!(v["priority"], "normal");
    assert_eq!(v["report_back"], true);
    assert_eq!(v["deadline_ms"], 60000);
    assert_eq!(v["context"]["dinner_time"], "7:00 PM");
}

/// message-types.md §delegate: priority defaults to "normal", report_back to true.
#[test]
fn delegate_defaults_per_spec() {
    let d: DelegatePayload = serde_json::from_value(json!({"task": "do something"})).unwrap();
    assert_eq!(d.priority, Priority::Normal);
    assert!(d.report_back);
    assert!(d.context.is_none());
    assert!(d.deadline_ms.is_none());
}

/// message-types.md §ack: accepted, estimated_ms.
#[test]
fn ack_payload_matches_spec() {
    let p = AckPayload {
        accepted: true,
        estimated_ms: Some(5000),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["accepted"], true);
    assert_eq!(v["estimated_ms"], 5000);
}

/// message-types.md §result: status, outcome, data, error.
#[test]
fn result_payload_matches_spec() {
    let p = ResultPayload {
        status: TaskStatus::Completed,
        outcome: "Message sent. Got a thumbs-up reaction.".to_string(),
        data: Some(json!({"reaction": "👍"})),
        error: None,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["status"], "completed");
    assert_eq!(v["outcome"], "Message sent. Got a thumbs-up reaction.");
    assert!(v.get("error").is_none() || v["error"].is_null());
}

/// message-types.md §notify: topic, data, importance.
#[test]
fn notify_payload_matches_spec() {
    let p = NotifyPayload {
        topic: "user.location".to_string(),
        data: json!({"status": "heading out", "eta_back": "2h"}),
        importance: Importance::Low,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["topic"], "user.location");
    assert_eq!(v["data"]["eta_back"], "2h");
    assert_eq!(v["importance"], "low");
}

/// message-types.md §notify: importance defaults to low.
#[test]
fn notify_importance_defaults_to_low() {
    let n: NotifyPayload = serde_json::from_value(json!({"topic": "t", "data": {}})).unwrap();
    assert_eq!(n.importance, Importance::Low);
}

/// message-types.md §cancel: reason field.
#[test]
fn cancel_payload_matches_spec() {
    let p = CancelPayload {
        reason: Some("Plans changed, no longer needed".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["reason"], "Plans changed, no longer needed");
}

/// message-types.md §discover: empty payload.
#[test]
fn discover_payload_is_empty() {
    let p = DiscoverPayload {};
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v, json!({}));
}

/// message-types.md §capabilities: all fields present.
#[test]
fn capabilities_payload_matches_spec() {
    let p = CapabilitiesPayload {
        agent_name: Some("Family Assistant".to_string()),
        domains: vec!["family".to_string(), "calendar".to_string()],
        channels: vec!["imessage".to_string()],
        tools: vec!["web_search".to_string(), "calendar_cli".to_string()],
        max_concurrent_tasks: Some(4),
        model: Some("gemini-3-pro".to_string()),
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["agent_name"], "Family Assistant");
    assert_eq!(v["domains"], json!(["family", "calendar"]));
    assert_eq!(v["channels"], json!(["imessage"]));
    assert_eq!(v["max_concurrent_tasks"], 4);
    assert_eq!(v["model"], "gemini-3-pro");
}

/// message-types.md §error: code, message, retryable.
#[test]
fn error_payload_matches_spec() {
    let p = ErrorPayload {
        code: ErrorCode::UnknownDomain,
        message: "I don't have access to work calendars. Try querying the work agent.".to_string(),
        retryable: false,
    };
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["code"], "unknown_domain");
    assert!(v["message"].as_str().unwrap().contains("work calendars"));
    assert_eq!(v["retryable"], false);
}
