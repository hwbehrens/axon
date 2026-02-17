pub fn print_annotated_examples() {
    println!(
        r#"AXON — Complete annotated example interactions
==============================================

LLMs learn from examples faster than from specifications.
Below is a full request → response and fire-and-forget messaging sequence.

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
Step 2: Send a request
──────────────────────────────────────────────
$ axon send ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "What is the capital of France?"

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"request","payload":{{"message":"What is the capital of France?"}}}}
  Wire message: {{
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "kind": "request",
    "payload": {{"message":"What is the capital of France?"}}
  }}
  Wire response: {{
    "id": "660e8400-e29b-41d4-a716-446655440001",
    "kind": "response",
    "ref": "550e8400-e29b-41d4-a716-446655440000",
    "payload": {{"answer": "Paris"}}
  }}

──────────────────────────────────────────────
Step 3: Send a fire-and-forget message
──────────────────────────────────────────────
$ axon notify ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 '{{"state":"ready"}}'

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"message","payload":{{"data":{{"state":"ready"}}}}}}
  IPC ack:      {{"ok":true,"msg_id":"..."}}
  (No wire response — message is unidirectional / fire-and-forget.)

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
→ {{"cmd":"subscribe","replay":false,"kinds":["request","message"],"req_id":"s1"}}
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
- Either side can initiate the QUIC connection; duplicates are resolved automatically.
- Messages are framed by QUIC stream FIN (no length prefix).
- Bidirectional streams are used for request/response patterns (kind: "request").
- Unidirectional streams are used for fire-and-forget messages (kind: "message").
- Identity is established by mTLS — peer identity is derived from the TLS certificate.
"#
    );
}
