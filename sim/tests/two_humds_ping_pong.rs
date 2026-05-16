//! Minimum routing proof: two humds, wired, one sends a tone targeting
//! the other's HumdId, the other's incoming-tone tap captures it.
//!
//! Narrative: humd-A and humd-B are peers in the same ensemble. From
//! humd-B's mock-nestler surface we want to inject a tone with
//! `to: <humd-A id hex>`. humd-B's ensemble routes it across the wired
//! `InMemoryEndpoint::pair` link; humd-A's HumdSink receives it.
//!
//! This test only validates the *transport seam* — no perch, no nest,
//! no prompt pipeline. If it fails, ensemble routing is broken.

use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn two_humds_ping_pong() {
    let _ = tracing_subscriber::fmt::try_init();

    let sim = sim::Sim::new();

    let a = sim.spawn_humd(ensemble::HumdId::random()).await;
    let b = sim.spawn_humd(ensemble::HumdId::random()).await;

    sim.wire(a.id, b.id).expect("wire humd-A and humd-B");

    // From humd-B's mock nestler, send a tone addressed to humd-A.
    // Any chi will do — this test is about routing, not semantics.
    let tone = serde_json::json!({
        "chi": "hello",
        "rid": "ping-1",
        "sid": "ping-sid",
        "to": a.id.to_hex(),
        "from": b.id.to_hex(),
    });

    // Drive the tone into humd-B as if a nestler had sent it. Humd-B's
    // ToneSink should recognise `to:` and hand it to its ensemble for
    // routing across the wire.
    sim.nestler_send(b.id, tone).expect("humd-B nestler accepts outbound tone");

    // humd-A should now see the routed tone. The natural way to assert
    // this is a "humd_incoming_tap" — a hook into humd-A's
    // ensemble→HumdSink seam that records every tone HumdSink received
    // from a peer (as opposed to from a local nestler). Today the sim
    // does not expose that hook directly.
    //
    // TODO(sim-api): add `Sim::humd_incoming_tap(humd, timeout) ->
    // Option<Value>` that returns the next tone humd-A's HumdSink saw
    // arrive via an ensemble peer. Until then, the closest available
    // surface is `nestler_recv`, which only sees tones broadcast on a
    // sid back to a registered synthetic client — not raw inbound
    // peer tones. If humd-A's HumdSink rebroadcasts received tones on
    // their sid (it should, for `prompt`/`chunk`/`finish` shaped chi),
    // we could observe them there; for a pure `chi:hello` ping it
    // currently won't surface to the nestler tap.
    //
    // For now we bind a synthetic client on humd-A so subsequent
    // tones tagged sid="ping-sid" would be observable, and assert the
    // round-trip *if* the daemon surfaces inbound peer tones on the
    // sid bus. If this assertion races / returns None, the integration
    // agent needs to wire the incoming-peer tap.
    let _ = sim.nestler_send(a.id, serde_json::json!({
        "chi": "subscribe",
        "rid": "sub-a",
        "sid": "ping-sid",
    })).expect("humd-A nestler subscribe");

    let got = sim
        .nestler_recv(a.id, "ping-sid", Duration::from_secs(1))
        .await;

    assert!(
        got.is_some(),
        "humd-A should observe a tone on sid=ping-sid within 1s — \
         requires incoming-peer-tone visibility via the sim's tap"
    );
    let got = got.unwrap();
    assert_eq!(
        got.get("rid").and_then(|v| v.as_str()),
        Some("ping-1"),
        "rid should pass through ensemble routing unchanged"
    );
}
