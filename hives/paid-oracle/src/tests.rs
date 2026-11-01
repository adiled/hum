use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::chain::*;

const PAY_TO: &str = "0x1111111111111111111111111111111111111111";
const USDC: &str = "0x2222222222222222222222222222222222222222";
const OTHER: &str = "0x3333333333333333333333333333333333333333";

fn settings(pairs: &[(&str, &str)]) -> Result<Config> {
    let vars: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Config::from_vars(|k| vars.get(k).cloned())
}

fn oracle(chain: &Chain, kind: PayKind) -> (Oracle, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let kind = kind.as_str();
    let cfg = settings(&[
        ("PAID_ORACLE_PAY_TO", PAY_TO),
        ("PAID_ORACLE_PAY_KIND", kind),
        ("PAID_ORACLE_USDC", USDC),
        ("PAID_ORACLE_RPC", &chain.rpc_url()),
        ("PAID_ORACLE_PRICE_URL", &chain.price_url()),
    ])
    .unwrap();
    let oracle = Oracle { cfg, state: State::open(dir.path()).unwrap(), http: reqwest::Client::new() };
    (oracle, dir)
}

fn tx(n: u8) -> String {
    format!("0x{}", hex::encode([n; 32]))
}

fn quote(proof: Option<(&str, &str)>) -> Value {
    let args = match proof {
        Some((tx, nonce)) => json!({ "paymentProof": { "txHash": tx, "nonce": nonce } }),
        None => json!({}),
    };
    json!({ "chi": "tendril-reach", "sid": "s1", "callId": "c1", "name": QUOTE_TOOL, "args": args })
}

async fn code(oracle: &Oracle, tx: &str, nonce: &str) -> String {
    let a = oracle.answer(&quote(Some((tx, nonce)))).await;
    if a.is_error { a.result["code"].as_str().unwrap().to_string() } else { "sold".into() }
}

#[test]
fn an_empty_setting_is_unset_and_every_setting_is_an_arg_of_the_orchfile() {
    let asked = Mutex::new(Vec::new());
    let cfg = Config::from_vars(|k| {
        asked.lock().unwrap().push(k.to_string());
        Some(if k == "PAID_ORACLE_PAY_TO" { PAY_TO } else { " " }.to_string())
    })
    .unwrap();
    assert_eq!(cfg.pay_kind, PayKind::Native);
    assert_eq!((cfg.decimals, cfg.price_atomic, cfg.confirmations), (18, 50_000_000_000_000_000, 1));
    assert_eq!(cfg.rpc_url, "https://rpc.testnet.arc.network");
    assert!(cfg.price_url.starts_with("https://api.coingecko.com/"));

    let orchfile = include_str!("../Orchfile");
    let asked = asked.into_inner().unwrap();
    assert!(asked.len() >= 8, "{asked:?}");
    for k in asked {
        assert!(orchfile.lines().any(|l| l == format!("ARG {k}=")), "no ARG {k} in the Orchfile");
        assert!(orchfile.lines().any(|l| l == format!("ENV {k}=${{{k}}}")), "no ENV {k} in the Orchfile");
    }
}

#[test]
fn a_setting_that_cannot_be_held_to_stops_the_bee() {
    let with = |k: &str, v: &str| settings(&[("PAID_ORACLE_PAY_TO", PAY_TO), (k, v)]);
    assert!(settings(&[]).is_err(), "PAID_ORACLE_PAY_TO is required");
    assert!(settings(&[("PAID_ORACLE_PAY_TO", "")]).is_err(), "an empty PAID_ORACLE_PAY_TO is none");
    assert!(with("PAID_ORACLE_PAY_TO", "0x1234").is_err());
    assert!(with("PAID_ORACLE_USDC", "usdc").is_err());
    assert!(with("PAID_ORACLE_PAY_KIND", "sol").is_err());
    assert!(with("PAID_ORACLE_DECIMALS", "six").is_err(), "a typo does not fall back to 18 decimals");
    assert!(with("PAID_ORACLE_DECIMALS", "1").is_err(), "a quote is never free");
    assert!(with("PAID_ORACLE_DECIMALS", "40").is_err());
    assert!(with("PAID_ORACLE_CONFIRMATIONS", "0").is_err());
    assert_eq!(with("PAID_ORACLE_DECIMALS", "6").unwrap().price_atomic, 50_000);
}

#[test]
fn every_answer_is_a_tendril_result_under_the_reachs_call_id() {
    let reach = quote(None);
    for answer in [refused(409, "spent"), refused(503, "later")] {
        let r = reply(&reach, answer);
        assert_eq!(r["chi"], "tendril-result");
        assert_eq!((r["callId"].as_str(), r["sid"].as_str()), (Some("c1"), Some("s1")));
        assert_eq!(r["isError"], true);
        assert!(!r["output"].as_str().unwrap().is_empty(), "a cell reads output: {r}");
        assert!(thrum_core::undeclared(&r).is_none(), "{r}");
    }
}

#[tokio::test]
async fn the_terms_and_any_other_tool_answer_the_reach() {
    let chain = Chain::start().await;
    let (oracle, _dir) = oracle(&chain, PayKind::Erc20);
    let terms = oracle.answer(&quote(None)).await;
    assert!(terms.is_error);
    assert_eq!(terms.result["code"], "402");
    let x402 = &terms.result["x402"];
    assert_eq!((x402["pay_to"].as_str(), x402["asset"].as_str()), (Some(PAY_TO), Some(USDC)));
    assert_eq!((x402["price_atomic"].as_str(), x402["confirmations"].as_u64()), (Some("50000"), Some(1)));
    assert!(oracle.state.issued(x402["nonce"].as_str().unwrap()));
    assert!(terms.output.starts_with("payment required"));

    let other = oracle.answer(&json!({ "name": "nope", "callId": "c2" })).await;
    assert_eq!(other.result["code"], "404");
}

#[test]
fn the_tool_takes_no_pair() {
    let cfg = settings(&[("PAID_ORACLE_PAY_TO", PAY_TO)]).unwrap();
    let h = hello(&cfg);
    let props = h["tools"][0]["inputSchema"]["properties"].as_object().unwrap();
    assert_eq!(props.keys().collect::<Vec<_>>(), ["paymentProof"]);
    assert!(h["tools"][0]["inputSchema"].get("required").is_none());
    assert_eq!(h["chis"], json!(["hello", "tendril-reach", "tendril-result"]));
}

#[tokio::test]
async fn a_pending_or_reverted_or_shallow_transaction_pays_nothing() {
    let chain = Chain::start().await;
    let (mut oracle, _dir) = oracle(&chain, PayKind::Native);
    let nonce = oracle.state.mint().unwrap();
    let pays = native(PAY_TO, 50_000_000_000_000_000, &input_with(&nonce));

    assert_eq!(code(&oracle, &tx(1), &nonce).await, "425", "a transaction the chain does not hold");
    chain.hold(&tx(1), pays.clone(), None);
    assert_eq!(code(&oracle, &tx(1), &nonce).await, "425", "a pending transaction");
    chain.hold(&tx(1), pays.clone(), Some(json!({ "status": "0x1", "blockNumber": null, "logs": [] })));
    assert_eq!(code(&oracle, &tx(1), &nonce).await, "425", "a receipt with no block");
    chain.hold(&tx(2), pays.clone(), Some(receipt("0x0", 100, vec![])));
    assert_eq!(code(&oracle, &tx(2), &nonce).await, "402", "a reverted transaction");
    chain.hold(&tx(3), pays.clone(), Some(receipt("0x1", 100, vec![])));
    oracle.cfg.confirmations = 3;
    assert_eq!(code(&oracle, &tx(3), &nonce).await, "425", "one confirmation of three");
    chain.head(102);
    assert_eq!(code(&oracle, &tx(3), &nonce).await, "sold", "three of three");
    assert_eq!(code(&oracle, &tx(4), &nonce).await, "425", "the oracle's refusals spent nothing");

    let down = Oracle { cfg: Config { rpc_url: "http://127.0.0.1:1".into(), ..oracle.cfg.clone() }, ..oracle };
    assert_eq!(code(&down, &tx(5), &nonce).await, "503");
}

#[tokio::test]
async fn a_native_payment_is_its_value_to_pay_to_carrying_the_nonce() {
    let chain = Chain::start().await;
    let (oracle, _dir) = oracle(&chain, PayKind::Native);
    let nonce = oracle.state.mint().unwrap();
    let price = oracle.cfg.price_atomic;
    let mined = || Some(receipt("0x1", 100, vec![]));

    chain.hold(&tx(1), native(OTHER, price, &input_with(&nonce)), mined());
    assert_eq!(code(&oracle, &tx(1), &nonce).await, "402", "paid to someone else");
    chain.hold(&tx(2), native(PAY_TO, price - 1, &input_with(&nonce)), mined());
    assert_eq!(code(&oracle, &tx(2), &nonce).await, "402", "underpaid");
    chain.hold(&tx(3), native(PAY_TO, price, "0x"), mined());
    assert_eq!(code(&oracle, &tx(3), &nonce).await, "402", "no nonce in the input");
    let shifted = format!("0x0{}0", input_with(&nonce).trim_start_matches("0x"));
    chain.hold(&tx(4), native(PAY_TO, price, &shifted), mined());
    assert_eq!(code(&oracle, &tx(4), &nonce).await, "402", "the nonce's hex half a byte off is not its bytes");
    chain.hold(&tx(5), native(&PAY_TO.to_uppercase().replace("0X", "0x"), price, &input_with(&nonce)), mined());
    assert_eq!(code(&oracle, &tx(5), &nonce).await, "sold");
}

#[tokio::test]
async fn an_erc20_payment_is_a_transfer_the_asset_logged_to_pay_to() {
    let chain = Chain::start().await;
    let (oracle, _dir) = oracle(&chain, PayKind::Erc20);
    let nonce = oracle.state.mint().unwrap();
    let price = oracle.cfg.price_atomic;
    let input = transfer_input(PAY_TO, price, &nonce);
    let paid = |n: u8, logs: Vec<Value>| chain.hold(&tx(n), call(USDC, &input), Some(receipt("0x1", 100, logs)));

    paid(1, vec![]);
    assert_eq!(code(&oracle, &tx(1), &nonce).await, "402", "calldata that names pay_to moves nothing by itself");
    paid(2, vec![transfer_log(OTHER, OTHER, PAY_TO, price)]);
    assert_eq!(code(&oracle, &tx(2), &nonce).await, "402", "another token's transfer");
    paid(3, vec![transfer_log(USDC, OTHER, OTHER, price)]);
    assert_eq!(code(&oracle, &tx(3), &nonce).await, "402", "a transfer to someone else");
    paid(4, vec![transfer_log(USDC, PAY_TO, PAY_TO, price)]);
    assert_eq!(code(&oracle, &tx(4), &nonce).await, "402", "pay_to paying itself");
    paid(5, vec![transfer_log(USDC, OTHER, PAY_TO, price - 1)]);
    assert_eq!(code(&oracle, &tx(5), &nonce).await, "402", "underpaid");
    paid(6, vec![transfer_log(USDC, OTHER, PAY_TO, price / 2), transfer_log(USDC, OTHER, PAY_TO, price - price / 2)]);
    assert_eq!(code(&oracle, &tx(6), &nonce).await, "sold", "two transfers that together pay");
    chain.hold(&tx(7), call(OTHER, &input_with(&nonce)), Some(receipt("0x1", 100, vec![transfer_log(USDC, OTHER, PAY_TO, price)])));
    assert_eq!(code(&oracle, &tx(7), &nonce).await, "sold", "a transfer made through another contract");
}

#[tokio::test]
async fn a_transaction_redeems_one_call_across_restarts() {
    let chain = Chain::start().await;
    let (oracle, dir) = oracle(&chain, PayKind::Native);
    let (a, b) = (oracle.state.mint().unwrap(), oracle.state.mint().unwrap());
    let both = format!("{}{}", input_with(&a), hex::encode(&b));
    chain.hold(&tx(1), native(PAY_TO, oracle.cfg.price_atomic, &both), Some(receipt("0x1", 100, vec![])));

    assert_eq!(code(&oracle, &tx(1), &a).await, "sold");
    assert_eq!(code(&oracle, &tx(1), &a).await, "409");
    assert_eq!(code(&oracle, &tx(1), &b).await, "409", "a second nonce in the same transaction buys nothing");
    assert_eq!(code(&oracle, &tx(1).to_uppercase().replace("0X", "0x"), &b).await, "409", "nor the hash in capitals");

    let reopened = Oracle { state: State::open(dir.path()).unwrap(), ..oracle };
    assert_eq!(code(&reopened, &tx(1), &b).await, "409", "a restart forgets no redemption");
    let c = reopened.state.mint().unwrap();
    assert!(reopened.state.issued(&a) && reopened.state.issued(&c), "a nonce outlives a restart");
}

#[tokio::test]
async fn a_nonce_this_oracle_never_issued_is_refused() {
    let chain = Chain::start().await;
    let (oracle, _dir) = oracle(&chain, PayKind::Native);
    let (stranger, _other) = self::oracle(&chain, PayKind::Native);
    let theirs = stranger.state.mint().unwrap();
    chain.hold(&tx(1), native(PAY_TO, oracle.cfg.price_atomic, &input_with(&theirs)), Some(receipt("0x1", 100, vec![])));
    assert_eq!(code(&oracle, &tx(1), &theirs).await, "410");
    assert_eq!(code(&oracle, &tx(1), "00").await, "410");
    assert_eq!(code(&oracle, "0x01", &theirs).await, "400");
    assert_eq!(code(&oracle, "../../nonce.key", &theirs).await, "400");
}

#[tokio::test]
async fn a_price_that_cannot_be_had_spends_nothing() {
    let chain = Chain::start().await;
    let (oracle, dir) = oracle(&chain, PayKind::Native);
    let nonce = oracle.state.mint().unwrap();
    chain.hold(&tx(1), native(PAY_TO, oracle.cfg.price_atomic, &input_with(&nonce)), Some(receipt("0x1", 100, vec![])));

    for (status, body) in [
        (500, r#"{"ethereum":{"usd":3456.78}}"#),
        (200, "not json"),
        (200, r#"{"bitcoin":{"usd":1.5},"ethereum":{"usd":3456.78}}"#),
        (200, r#"{"ethereum":{"usd":3456.78,"eur":3000}}"#),
        (200, r#"{"ethereum":{"usd":0}}"#),
        (200, r#"{"ethereum":{"usd":"3456.78"}}"#),
        (200, r#"{}"#),
    ] {
        chain.price(status, body);
        let a = oracle.answer(&quote(Some((&tx(1), &nonce)))).await;
        assert_eq!(a.result["code"], "503", "{status} {body}: {a:?}");
        assert!(a.output.contains("not spent"));
        assert!(!dir.path().join("redeemed").join(tx(1)).exists());
    }
    chain.price(200, r#"{"ethereum":{"usd":3456.78}}"#);
    let sold = oracle.answer(&quote(Some((&tx(1), &nonce)))).await;
    assert!(!sold.is_error, "{sold:?}");
    assert_eq!(sold.result["px"], "3456.78");
    assert_eq!(sold.result["source"], chain.price_url());
    assert!(sold.output.contains("3456.78"));
}

#[tokio::test]
async fn an_unpaid_reach_never_reaches_the_price() {
    let chain = Chain::start().await;
    let (oracle, _dir) = oracle(&chain, PayKind::Native);
    let nonce = oracle.state.mint().unwrap();
    chain.hold(&tx(1), native(PAY_TO, 1, &input_with(&nonce)), Some(receipt("0x1", 100, vec![])));
    assert_eq!(code(&oracle, &tx(1), &nonce).await, "402");
    oracle.answer(&quote(None)).await;
    assert_eq!(chain.price_hits(), 0);
}

#[test]
fn a_quantity_saturates_and_refuses_what_is_not_hex() {
    assert_eq!(uint(&json!("0x0")), Some(0));
    assert_eq!(uint(&json!("0x")), Some(0));
    assert_eq!(uint(&json!("0x00ff")), Some(255));
    assert_eq!(uint(&json!(format!("0x1{}", "0".repeat(40)))), Some(u128::MAX));
    assert_eq!(uint(&json!("0xzz")), None);
    assert_eq!(uint(&json!("12")), None);
    assert_eq!(uint(&json!(12)), None);
}
