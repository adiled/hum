use rand::RngCore;
use serde_json::Value;

use crate::{Hid, HumdAddr, Tone};

use super::{KAD_FIND_NODE_CHI, KAD_FIND_NODE_RESP_CHI};

pub fn mint_query_id() -> String {
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn find_node_tone(rid: &str, query_id: &str, target: &Hid, from: &Hid) -> Tone {
    serde_json::json!({
        "chi": KAD_FIND_NODE_CHI,
        "rid": rid,
        "query_id": query_id,
        "target": target.to_hex(),
        "from": from.to_hex(),
    })
}

pub fn find_node_resp_tone(rid: &str, query_id: &str, from: &Hid, closest: &[HumdAddr]) -> Tone {
    serde_json::json!({
        "chi": KAD_FIND_NODE_RESP_CHI,
        "rid": rid,
        "query_id": query_id,
        "from": from.to_hex(),
        "closest": closest,
    })
}

pub struct ParsedFindNode {
    pub query_id: String,
    pub target: Hid,
    pub from: Hid,
    pub rid: String,
}

pub struct ParsedFindNodeResp {
    pub query_id: String,
    pub from: Hid,
    pub closest: Vec<HumdAddr>,
}

fn parse_humd_id(v: &Value) -> Option<Hid> {
    Hid::from_hex(v.as_str()?).ok()
}

pub fn parse_find_node(tone: &Tone) -> Option<ParsedFindNode> {
    let query_id = tone.get("query_id")?.as_str()?.to_string();
    let target = parse_humd_id(tone.get("target")?)?;
    let from = parse_humd_id(tone.get("from")?)?;
    let rid = tone
        .get("rid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(ParsedFindNode {
        query_id,
        target,
        from,
        rid,
    })
}

pub fn parse_find_node_resp(tone: &Tone) -> Option<ParsedFindNodeResp> {
    let query_id = tone.get("query_id")?.as_str()?.to_string();
    let from = parse_humd_id(tone.get("from")?)?;
    let closest_json = tone.get("closest")?.as_array()?;
    let closest: Vec<HumdAddr> = closest_json
        .iter()
        .filter_map(|v| serde_json::from_value::<HumdAddr>(v.clone()).ok())
        .collect();
    Some(ParsedFindNodeResp {
        query_id,
        from,
        closest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_find_node_round_trip() {
        let me = Hid::random_humd();
        let target = Hid::random_humd();
        let q = mint_query_id();
        let tone = find_node_tone("rid-1", &q, &target, &me);
        let parsed = parse_find_node(&tone).expect("parse");
        assert_eq!(parsed.query_id, q);
        assert_eq!(parsed.target, target);
        assert_eq!(parsed.from, me);
    }

    #[test]
    fn parse_find_node_resp_round_trip() {
        let me = Hid::random_humd();
        let peer = HumdAddr::new(Hid::random_humd()).with_hint("tcp:1.2.3.4:9000");
        let tone = find_node_resp_tone("rid-1", "qid-1", &me, std::slice::from_ref(&peer));
        let parsed = parse_find_node_resp(&tone).expect("parse");
        assert_eq!(parsed.query_id, "qid-1");
        assert_eq!(parsed.from, me);
        assert_eq!(parsed.closest.len(), 1);
        assert_eq!(parsed.closest[0].id, peer.id);
        assert_eq!(parsed.closest[0].hints, peer.hints);
    }
}
