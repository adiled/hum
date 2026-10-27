//! The forager as an Ollama client meets it: the models it lists, a
//! conversation carried whole, a tool call answered in a second request,
//! a bloom that breaks off, a client that goes away, and the one link all
//! of it shares.

mod common;

use std::time::Duration;

use common::*;
use serde_json::{json, Value};

fn names(tags: &Value) -> Vec<String> {
    tags["models"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap().to_string()).collect()
}

fn weather() -> Value {
    json!([{"type": "function", "function": {"name": "weather", "description": "the weather in a city",
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}])
}

#[tokio::test(flavor = "multi_thread")]
async fn tags_list_what_the_breath_reaches() {
    let f = Forager::start("tags", &[("OLLAMA_SERVER_MODELS", "")]).await;
    let tags = get(&f.url("/api/tags")).await;
    assert_eq!(names(&tags), [MODEL], "the models humd's nurses grow, an empty setting being no setting");

    let f = Forager::start("tags-set", &[("OLLAMA_SERVER_MODELS", "a, b")]).await;
    let tags = get(&f.url("/api/tags")).await;
    assert_eq!(names(&tags), ["a", "b"], "the configured list overrides the breath");
}

#[tokio::test(flavor = "multi_thread")]
async fn earlier_turns_ride_as_the_graft() {
    let f = Forager::start("graft", &[]).await;
    let (status, body) = post(&f.url("/api/chat"), json!({
        "model": MODEL, "stream": false,
        "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "graft-one"},
            {"role": "assistant", "content": "the first answer"},
            {"role": "user", "content": "graft-two"},
        ],
    })).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["message"]["content"], "echo: graft-two");
    let prompt = world().prompt("graft-two").await;
    assert_eq!(prompt["systemPrompt"], "be brief");
    assert_eq!(words_of(&prompt), "graft-two");
    assert_eq!(prompt["graft"], json!([{
        "content": [{"kind": "text", "text": "graft-one"}],
        "grew": [{"kind": "text", "text": "the first answer"}],
    }]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_keeps_its_sigil_while_it_grows() {
    let f = Forager::start("sigil", &[]).await;
    let first = json!({"role": "user", "content": "sigil-one"});
    let (_, one) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false, "messages": [first]})).await;
    let (_, _) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false, "messages": [
        first, one["message"], {"role": "user", "content": "sigil-two"},
    ]})).await;
    let (_, _) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false, "messages": [
        {"role": "user", "content": "sigil-three"},
    ]})).await;
    let sid = |marker| async move { world().prompt(marker).await["sid"].as_str().unwrap().to_string() };
    assert_eq!(sid("sigil-one").await, sid("sigil-two").await, "the next turn grows on the same sigil");
    assert_ne!(sid("sigil-one").await, sid("sigil-three").await, "another conversation gets its own");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_call_completes_in_the_next_request() {
    let f = Forager::start("tools", &[]).await;
    let ask = json!({"role": "user", "content": "tool weather {\"city\":\"tools-paris\"}"});
    let (status, lines) = lines(&f.url("/api/chat"), json!({"model": MODEL, "tools": weather(), "messages": [ask]})).await;
    assert_eq!(status, 200);
    let called = lines.iter().find(|l| l["message"]["tool_calls"].is_array()).expect("a tool_calls line");
    let call = &called["message"]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "weather");
    assert_eq!(call["function"]["arguments"], json!({"city": "tools-paris"}));
    let last = lines.last().unwrap();
    assert_eq!((last["done"].clone(), last["done_reason"].clone()), (json!(true), json!("stop")), "the turn goes back to the client");

    let sid = world().prompt("tools-paris").await["sid"].as_str().unwrap().to_string();
    let (status, body) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false, "tools": weather(), "messages": [
        ask,
        {"role": "assistant", "content": "", "tool_calls": [call]},
        {"role": "tool", "tool_call_id": call["id"], "content": "sunny"},
    ]})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["message"]["content"], "grasped: sunny");
    let results = world().results(&sid);
    assert_eq!(results.len(), 1, "the answer goes to the held reach: {results:?}");
    assert_eq!(results[0]["output"], "sunny");
    assert_eq!(world().prompts("tools-paris").len(), 1, "the bloom grows on, no new prompt");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_answer_without_ids_finds_its_reach() {
    let f = Forager::start("bare-tools", &[]).await;
    let ask = json!({"role": "user", "content": "tool weather {\"city\":\"bare-oslo\"}"});
    let (_, body) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false, "tools": weather(), "messages": [ask]})).await;
    let function = body["message"]["tool_calls"][0]["function"].clone();
    assert_eq!(function["name"], "weather", "{body}");

    let (status, body) = lines(&f.url("/api/chat"), json!({"model": MODEL, "tools": weather(), "messages": [
        ask,
        {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": function["name"], "arguments": function["arguments"]}}]},
        {"role": "tool", "tool_name": "weather", "content": "snow"},
    ]})).await;
    assert_eq!(status, 200);
    let words: String = body.iter().filter_map(|l| l["message"]["content"].as_str()).collect();
    assert_eq!(words, "grasped: snow");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_bloom_answers_with_an_error_status() {
    let f = Forager::start("errors", &[]).await;
    let (status, body) = post(&f.url("/api/chat"), json!({"model": MODEL, "stream": false,
        "messages": [{"role": "user", "content": "fail-collected"}]})).await;
    assert_eq!(status, 502, "{body}");
    assert_eq!(body["error"], "the cell broke");

    let (status, _) = lines(&f.url("/api/chat"), json!({"model": MODEL, "messages": [{"role": "user", "content": "fail-streamed"}]})).await;
    assert_eq!(status, 502, "a stream that breaks off before its first word fails with its status");

    let (status, body) = post(&f.url("/api/generate"), json!({"model": MODEL, "stream": false, "prompt": "fail-generated"})).await;
    assert_eq!(status, 502, "{body}");

    let (status, body) = post(&f.url("/api/chat"), json!({"model": "no-such-model", "stream": false,
        "messages": [{"role": "user", "content": "nowhere"}]})).await;
    assert_eq!(status, 404, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_link_serves_every_request() {
    let f = Forager::start("links", &[]).await;
    let Some(before) = f.sockets() else {
        eprintln!("no lsof here; the forager's links go uncounted");
        return;
    };
    for i in 0..4 {
        let (status, _) = post(&f.url("/api/generate"), json!({"model": MODEL, "stream": false, "prompt": format!("links-{i}")})).await;
        assert_eq!(status, 200);
    }
    let links = f.sockets().unwrap() - before;
    assert_eq!(links, 1, "every request shares one link to humd, and none stays open after its wilt");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_goes_away_cancels_its_bloom() {
    let f = Forager::start("gone", &[]).await;
    let mut resp = client()
        .post(f.url("/api/chat"))
        .json(&json!({"model": MODEL, "messages": [{"role": "user", "content": "hang-streamed"}]}))
        .send()
        .await
        .unwrap();
    let first = resp.chunk().await.unwrap().expect("a first line");
    assert!(String::from_utf8_lossy(&first).contains("hanging"));
    drop(resp);
    let sid = world().prompt("hang-streamed").await["sid"].as_str().unwrap().to_string();
    assert!(until(|| world().cancelled(&sid).then_some(())).await.is_some(), "a stream dropped mid-bloom cancels it");

    let sent = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(f.url("/api/chat"))
        .timeout(Duration::from_millis(1500))
        .json(&json!({"model": MODEL, "stream": false, "messages": [{"role": "user", "content": "hang-collected"}]}))
        .send()
        .await;
    assert!(sent.is_err(), "the bloom hangs until the client gives up");
    let sid = world().prompt("hang-collected").await["sid"].as_str().unwrap().to_string();
    assert!(until(|| world().cancelled(&sid).then_some(())).await.is_some(), "a request dropped mid-bloom cancels it");
}
