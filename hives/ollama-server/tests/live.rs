//! The hive whole against a real Ollama: the forager on its own port, the
//! nurse on its default, Ollama's own port, and a model Ollama has pulled.
//!
//! OLLAMA_LIVE_MODEL=llama3.2 cargo test --test live -- --ignored

mod common;

use common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Ollama on 11434 and OLLAMA_LIVE_MODEL pulled into it"]
async fn a_conversation_grows_through_ollama() {
    let model = std::env::var("OLLAMA_LIVE_MODEL").expect("OLLAMA_LIVE_MODEL names a model Ollama has");
    let _nurse = spawn_nurse(&[("OLLAMA_NURSE_MODELS", &model)]).await;
    let f = Forager::start("live", &[]).await;

    let tags = get(&f.url("/api/tags")).await;
    assert!(tags["models"].as_array().unwrap().iter().any(|m| m["name"] == model.as_str()), "{tags}");

    let ask = json!({"role": "user", "content": "Reply with exactly one word: apple"});
    let (status, lines) = lines(&f.url("/api/chat"), json!({"model": model, "messages": [ask]})).await;
    assert_eq!(status, 200);
    let said: String = lines.iter().filter_map(|l| l["message"]["content"].as_str()).collect();
    assert!(lines.last().unwrap()["done"] == true && !said.is_empty(), "{lines:?}");

    let (status, body) = post(&f.url("/api/chat"), json!({"model": model, "stream": false, "messages": [
        ask,
        {"role": "assistant", "content": said},
        {"role": "user", "content": "Which fruit did you just name? Reply with that one word."},
    ]})).await;
    assert_eq!(status, 200, "{body}");
    let answer = body["message"]["content"].as_str().unwrap().to_lowercase();
    assert!(answer.contains("apple"), "the earlier turns reached the model: {answer:?}");

    let (status, body) = post(&f.url("/api/chat"), json!({"model": "no-such-model:1b", "stream": false,
        "messages": [{"role": "user", "content": "hi"}]})).await;
    assert_eq!(status, 404, "{body}");
}
