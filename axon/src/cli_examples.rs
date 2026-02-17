pub fn print_annotated_examples() {
    println!(
        r#"AXON — Complete annotated example interactions
==============================================

LLMs learn from examples faster than from specifications.
Below is a full request → response and fire-and-forget messaging sequence.

Agent IDs used:
  Alice: ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4
  Bob:   ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

Configuration — Static peers (config.toml)
──────────────────────────────────────────────
# ~/.axon/config.toml (Alice's machine)
name = "alice"

[[peers]]
agent_id = "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3"
addr = "192.168.1.42:7100"
pubkey = "<Bob's public_key from `axon identity`>"

Tip: run `axon identity --peer-config` to print a ready-to-share snippet.

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
        "agent_id": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
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
    "kind": "error",
    "ref": "550e8400-e29b-41d4-a716-446655440000",
    "payload": {{"code":"unhandled","message":"no application handler installed"}}
  }}
  (If the remote agent has an app handler, it may return a normal "response" instead.)

──────────────────────────────────────────────
Step 3: Send a fire-and-forget message
──────────────────────────────────────────────
$ axon notify ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 '{{"state":"ready"}}'

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"message","payload":{{"data":{{"state":"ready"}}}}}}
  IPC ack:      {{"ok":true,"msg_id":"..."}}
  (No wire response — message is unidirectional / fire-and-forget.)

──────────────────────────────────────────────
IPC Commands — Raw JSON (Unix socket)
──────────────────────────────────────────────

All examples below are newline-delimited JSON sent over ~/.axon/axon.sock.
All connected clients receive inbound messages as broadcast events.

# 1. Send a request (bidirectional — waits for response)
→ {{"cmd":"send","to":"ed25519.f6e5d4c3...","kind":"request","payload":{{"message":"What is 2+2?"}}}}
← {{"ok":true,"msg_id":"550e8400-...","response":{{"id":"660e8400-...","kind":"error","ref":"550e8400-...","payload":{{"code":"unhandled","message":"no application handler installed"}}}}}}

# 2. Send a fire-and-forget message (unidirectional)
→ {{"cmd":"send","to":"ed25519.f6e5d4c3...","kind":"message","payload":{{"data":{{"state":"ready"}}}}}}
← {{"ok":true,"msg_id":"770e8400-..."}}

# 3. List peers
→ {{"cmd":"peers"}}
← {{"ok":true,"peers":[{{"agent_id":"ed25519.f6e5d4c3...","addr":"192.168.1.42:7100","status":"connected","rtt_ms":1.23,"source":"static"}}]}}

# 4. Daemon status
→ {{"cmd":"status"}}
← {{"ok":true,"uptime_secs":3600,"peers_connected":1,"messages_sent":42,"messages_received":38}}

# 5. Daemon identity
→ {{"cmd":"whoami"}}
← {{"ok":true,"agent_id":"ed25519.a1b2...","public_key":"<base64>","name":"my-agent","version":"<version>","uptime_secs":3600}}

# 6. Inbound message event (broadcast to connected clients; lagging clients may be disconnected)
← {{"event":"inbound","from":"ed25519.f6e5d4c3...","envelope":{{"id":"880e8400-...","kind":"request","payload":{{"question":"Hello?"}}}}}}

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
