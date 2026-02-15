# AXON — Agent eXchange Over Network

A hyper-efficient, LLM-first local messaging protocol for agent-to-agent communication.

## Problem

Current inter-agent communication options are all designed for humans:
- iMessage: rich media, typing indicators, read receipts — none of which LLMs need
- HTTP/REST: stateless, no session awareness, JSON overhead
- OpenClaw sessions_send: works but goes through the gateway abstraction layer

We want something purpose-built for two LLM agents on the same local network.

## Design Principles

1. **Context-budget-aware**: Every message costs tokens. The protocol should minimize unnecessary context consumption.
2. **Structured-first**: No natural language overhead. Payloads are typed, schemaed, and machine-parseable.
3. **Resumable**: Agents restart frequently. The protocol handles reconnection, deduplication, and state recovery.
4. **Minimal round-trips**: Prefer rich single exchanges over chatty back-and-forth.
5. **Zero-trust locally**: Agents authenticate even on LAN (agents have different access levels).

## Status

Design phase. See `spec/` for the protocol specification and message type definitions.
