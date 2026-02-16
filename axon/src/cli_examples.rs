pub fn print_annotated_examples() {
    println!(
        r#"AXON — Complete annotated example interactions
==============================================

LLMs learn from examples faster than from specifications.
Below is a full hello → discover → query → delegate → cancel → notify sequence.

Agent IDs used:
  Alice: ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4  (lower — initiates connection)
  Bob:   ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

Network Protocol (QUIC)
──────────────────────────────────────────────
The following steps show the network-level QUIC protocol interaction.

──────────────────────────────────────────────
Step 0: Start the daemon
──────────────────────────────────────────────
$ axon daemon --port 7100

  INFO starting AXON daemon agent_id=ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4 port=7100

  (The daemon binds QUIC on 0.0.0.0:7100, creates ~/.axon/axon.sock for IPC,
   and begins connecting to any peers listed in ~/.axon/config.toml.)

──────────────────────────────────────────────
Step 1: List known peers
──────────────────────────────────────────────
$ axon peers

  IPC sent:     {{"cmd":"peers"}}
  IPC response: {{
    "ok": true,
    "peers": [
      {{
        "id": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "addr": "192.168.1.42:7100",
        "status": "connected",
        "rtt_ms": 1.23,
        "source": "static"
      }}
    ]
  }}

──────────────────────────────────────────────
Step 2: Discover peer capabilities
──────────────────────────────────────────────
$ axon discover ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"discover","payload":{{}}}}
  IPC ack:      {{"ok":true,"msg_id":"550e8400-e29b-41d4-a716-446655440000"}}
  Wire message: {{
    "v": 1,
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "from": "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
    "to": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
    "ts": 1771108000000,
    "kind": "discover",
    "payload": {{}}
  }}
  Wire response: {{
    "v": 1,
    "id": "660e8400-e29b-41d4-a716-446655440001",
    "from": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
    "to": "ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
    "ts": 1771108000050,
    "kind": "capabilities",
    "ref": "550e8400-e29b-41d4-a716-446655440000",
    "payload": {{
      "agent_name": "Bob's Research Assistant",
      "domains": ["web_search", "summarization"],
      "tools": ["web_search", "pdf_reader"],
      "max_concurrent_tasks": 4
    }}
  }}

──────────────────────────────────────────────
Step 3: Send a query
──────────────────────────────────────────────
$ axon send ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "What is the capital of France?"

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"query","payload":{{"question":"What is the capital of France?","domain":"meta.query","max_tokens":200,"deadline_ms":30000}}}}
  Wire response: {{
    "v": 1,
    "kind": "response",
    "ref": "<msg_id>",
    "payload": {{
      "data": {{"answer": "Paris"}},
      "summary": "The capital of France is Paris.",
      "tokens_used": 12,
      "truncated": false
    }}
  }}

──────────────────────────────────────────────
Step 4: Delegate a task
──────────────────────────────────────────────
$ axon delegate ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "Summarize today's tech news"

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"delegate","payload":{{"task":"Summarize today's tech news","priority":"normal","report_back":true,"deadline_ms":60000}}}}
  Wire response (immediate ack): {{
    "v": 1,
    "kind": "ack",
    "ref": "<msg_id>",
    "payload": {{"accepted": true, "estimated_ms": 15000}}
  }}
  Wire response (later, via unidirectional stream): {{
    "v": 1,
    "kind": "result",
    "ref": "<msg_id>",
    "payload": {{
      "status": "completed",
      "outcome": "Here are today's top tech stories: ...",
      "data": {{"articles": 5}}
    }}
  }}

──────────────────────────────────────────────
Step 5: Cancel a delegated task
──────────────────────────────────────────────
$ axon cancel ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 --ref 550e8400-e29b-41d4-a716-446655440000 --reason "No longer needed"

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"cancel","payload":{{"reason":"No longer needed"}},"ref":"550e8400-e29b-41d4-a716-446655440000"}}
  Wire response: {{
    "v": 1,
    "kind": "ack",
    "ref": "<msg_id>",
    "payload": {{"accepted": true}}
  }}

──────────────────────────────────────────────
Step 6: Send a notification (fire-and-forget)
──────────────────────────────────────────────
$ axon notify ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 meta.status '{{"state":"ready"}}'

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"notify","payload":{{"topic":"meta.status","data":{{"state":"ready"}},"importance":"low"}}}}
  IPC ack:      {{"ok":true,"msg_id":"..."}}
  (No wire response — notify is unidirectional / fire-and-forget.)

──────────────────────────────────────────────
IPC v2 — Raw JSON (Unix socket)
──────────────────────────────────────────────

IPC v2 adds hello handshake, auth, req_id correlation, and subscribe.
All examples below are newline-delimited JSON sent over ~/.axon/axon.sock.

# 1. Hello handshake (required before v2 commands)
→ {{"cmd":"hello","version":2,"consumer":"my-agent","req_id":"h1"}}
← {{"ok":true,"version":2,"daemon_max_version":2,"agent_id":"ed25519.a1b2...","features":["auth","buffer","subscribe"],"req_id":"h1"}}

# 2. Auth (required if peer credentials unavailable)
→ {{"cmd":"auth","token":"<64-hex-chars-from-~/.axon/ipc-token>","req_id":"a1"}}
← {{"ok":true,"auth":"accepted","req_id":"a1"}}

# 3. Subscribe (live push, no replay)
→ {{"cmd":"subscribe","replay":false,"kinds":["query","delegate"],"req_id":"s1"}}
← {{"ok":true,"subscribed":true,"replayed":0,"replay_to_seq":42,"req_id":"s1"}}
← {{"event":"inbound","replay":false,"seq":43,"buffered_at_ms":1771108300123,"envelope":{{...}}}}

# 4. Inbox (pull-based retrieval)
→ {{"cmd":"inbox","limit":10,"req_id":"i1"}}
← {{"ok":true,"messages":[{{"seq":43,"buffered_at_ms":1771108300123,"envelope":{{...}}}}],"next_seq":43,"has_more":false,"req_id":"i1"}}

# 5. Ack (advance cursor)
→ {{"cmd":"ack","up_to_seq":43,"req_id":"k1"}}
← {{"ok":true,"acked_seq":43,"req_id":"k1"}}

# 6. Whoami (identity query)
→ {{"cmd":"whoami","req_id":"w1"}}
← {{"ok":true,"agent_id":"ed25519.a1b2...","public_key":"<base64>","version":"0.1.0","ipc_version":2,"uptime_secs":3600,"req_id":"w1"}}

──────────────────────────────────────────────
Notes
──────────────────────────────────────────────
- The lower agent_id always initiates the QUIC connection (initiator rule).
- Messages are framed by QUIC stream FIN (no length prefix).
- Bidirectional streams are used for request-response (hello, ping, query, delegate, cancel, discover).
- Unidirectional streams are used for fire-and-forget (notify, result).
- The hello handshake must complete before any other messages on a connection.
"#
    );
}
