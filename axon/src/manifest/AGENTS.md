# manifest — capability manifests, `describe` answering, and `who_can`

Status: normative for behavior; spec files are authoritative for wire/protocol rules.

Self-described service catalogs exchanged via the fifth message kind
`describe`. Manifests are **claims**: publication is opt-in, absence is
explicit (`no_manifest`), and they never affect TLS trust, pinning, or
enrollment. Only exercising a service validates a claim.

## Layout

- `mod.rs` — module root and re-exports.
- `types.rs` — `Manifest`/`ServiceEntry` schema, validation bounds
  (`MAX_MANIFEST_BYTES` 32 KiB, `MAX_SERVICES` 64), custom `Deserialize` via
  `try_from` so invalid manifests cannot exist in daemon state. Tests in
  `types_tests.rs`.
- `cache.rs` — bounded, TTL-aware runtime cache of remote peer manifests used
  by `who_can` and the `peers` advisory summary. Runtime-only: no durable
  store, entries age out. Tests in `cache_tests.rs`.

## Non-obvious rules

- The *daemon never authors* a manifest. The IPC handler publishes one at
  `serve` time; `RequestBroker` stores it with the lease; the broker's
  `describe` submodule answers inbound describes from it without waking the
  handler.
- `describe` bypasses the completed-response tombstone cache: it is
  side-effect free, so a fresh answer is always correct across manifest
  refreshes.
- Validation (schema bounds AND the 32 KiB encoded-size bound) runs at
deserialization (`from_parts`), so a parsed manifest always satisfies every
daemon invariant — on the `serve` path and the remote `describe`-response
path alike.
- Spec co-changes: `spec/MESSAGE_TYPES.md` (kind semantics), `spec/WIRE_FORMAT.md`
  §6.5 (manifest schema), `spec/IPC.md` §4.7/§4.9 (`serve`/`who_can`).
