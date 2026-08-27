use std::time::Instant;

use super::*;

#[tokio::test]
async fn claim_succeeds_again_after_abandoned_attempt() {
    let book = ReconnectBook::default();
    let peer: AgentId = crate::message::AgentId::parse(&format!("ed25519.{}", "a".repeat(32)))
        .expect("valid Agent ID");

    let ticket = book
        .claim(peer.clone(), Instant::now())
        .await
        .expect("first claim");
    // A second claim while in flight must be refused...
    assert!(book.claim(peer.clone(), Instant::now()).await.is_none());

    // ...but an attempt cancelled without a dial outcome (shutdown or peer
    // revocation) releases the slot: without `abandoned` the entry would
    // stay in_flight forever and maintenance could never claim another
    // attempt after re-enrollment.
    book.abandoned(&peer, ticket).await;
    assert!(
        book.claim(peer.clone(), Instant::now()).await.is_some(),
        "abandoned attempt must be reclaimable"
    );
}

#[tokio::test]
async fn abandoned_with_stale_ticket_leaves_newer_attempt_in_flight() {
    let book = ReconnectBook::default();
    let peer: AgentId = crate::message::AgentId::parse(&format!("ed25519.{}", "b".repeat(32)))
        .expect("valid Agent ID");

    let stale = book
        .claim(peer.clone(), Instant::now())
        .await
        .expect("first claim");
    book.succeeded(&peer, stale).await;

    // Version advanced past the stale ticket: a late abandoned() call from
    // the old attempt generation must not release the newer claim.
    let fresh = book
        .claim(peer.clone(), Instant::now())
        .await
        .expect("fresh claim");
    book.abandoned(&peer, stale).await;
    assert!(book.claim(peer.clone(), Instant::now()).await.is_none());

    book.abandoned(&peer, fresh).await;
    assert!(book.claim(peer.clone(), Instant::now()).await.is_some());
}
