//! The bundle side: what a datagram that arrives is, and the bundles an
//! answer goes home in.
//!
//! An arrival is an ask, a piece of one, or dropped for a reason. It is
//! dropped when it does not decode, when a CRC fails, when RFC 9171 does
//! not hold for it, when it is for another endpoint, when it is an
//! administrative record, when it is hum's own answer, when it names no
//! source to answer, and when its lifetime is over (§5.5). Its id is the
//! source and creation timestamp, which is the same for every piece and
//! every copy of it.
//!
//! An answer goes home as one bundle to the asker's endpoint, with a CRC
//! on every block (§4.3.1), the lifetime the ask was given, no status
//! reports asked for, and block 193 saying it answers the ask's id. An
//! answer longer than the datagram allows is cut into fragments that each
//! fit (§5.8; RFC 7122 §3.2), every one of them carrying block 193.

use std::time::Duration;

use anyhow::{bail, Result};
use bundle_protocol::canonical::{
    new_hop_count_block, new_payload_block, CanonicalBlock, CanonicalBlockBuilder, CanonicalData,
    BUNDLE_AGE_BLOCK,
};
use bundle_protocol::crc::CRC_32;
use bundle_protocol::flags::{BlockControlFlags, BundleControlFlags};
use bundle_protocol::primary::PrimaryBlockBuilder;
use bundle_protocol::{Bundle, CreationTimestamp, EndpointID};
use serde_json::{json, Value};

pub const HUM_BLOCK: u64 = 193;
pub const HOP_LIMIT: u8 = 32;
pub const MOST_ASK: u64 = 4 * 1024 * 1024;
const MS_1970_TO_2000: u64 = 946_684_800_000;
const MOST_HEADER: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub id: String,
    pub source: String,
    pub expires: u64,
    pub lifetime_ms: u64,
}

#[derive(Debug)]
pub enum Arrival {
    Whole(Ask, Vec<u8>),
    Piece { ask: Ask, offset: u64, total: u64, data: Vec<u8> },
    Dropped(&'static str),
}

pub fn arrive(bytes: &[u8], node_eid: &str, now_ms: u64) -> Arrival {
    let Ok(mut bundle) = Bundle::try_from(bytes) else {
        return Arrival::Dropped("undecodable");
    };
    if !bundle.crc_valid() {
        return Arrival::Dropped("crc");
    }
    if bundle.validate().is_err() {
        return Arrival::Dropped("invalid");
    }
    if bundle.primary.destination.to_string() != node_eid {
        return Arrival::Dropped("not-ours");
    }
    if bundle.is_administrative_record() {
        return Arrival::Dropped("administrative-record");
    }
    if is_reply(&bundle) {
        return Arrival::Dropped("hum-reply");
    }
    if bundle.primary.source == EndpointID::none() {
        return Arrival::Dropped("anonymous");
    }
    let lifetime_ms = bundle.primary.lifetime.as_millis() as u64;
    let created = bundle.primary.creation_timestamp.dtntime();
    let expires = if created != 0 {
        MS_1970_TO_2000.saturating_add(created).saturating_add(lifetime_ms)
    } else {
        now_ms.saturating_add(lifetime_ms).saturating_sub(age_ms(&bundle))
    };
    if expires <= now_ms {
        return Arrival::Dropped("expired");
    }
    let Some(data) = bundle.payload().cloned() else {
        return Arrival::Dropped("no-payload");
    };
    let ts = &bundle.primary.creation_timestamp;
    let source = bundle.primary.source.to_string();
    let ask = Ask { id: format!("{source}-{}-{}", ts.dtntime(), ts.seqno()), source, expires, lifetime_ms };
    if bundle.primary.has_fragmentation() {
        let offset = bundle.primary.fragmentation_offset;
        let total = bundle.primary.total_data_length;
        if offset.saturating_add(data.len() as u64) > total {
            return Arrival::Dropped("piece-outside-its-whole");
        }
        if total > MOST_ASK {
            return Arrival::Dropped("too-large");
        }
        return Arrival::Piece { ask, offset, total, data };
    }
    Arrival::Whole(ask, data)
}

fn age_ms(bundle: &Bundle) -> u64 {
    bundle
        .canonicals
        .iter()
        .find(|b| b.block_type == BUNDLE_AGE_BLOCK)
        .and_then(|b| match b.data() {
            CanonicalData::BundleAge(ms) => Some(*ms),
            _ => None,
        })
        .unwrap_or(0)
}

fn is_reply(bundle: &Bundle) -> bool {
    bundle.canonicals.iter().any(|b| {
        b.block_type == HUM_BLOCK
            && matches!(b.data(), CanonicalData::Unknown(bytes)
                if serde_json::from_slice::<Value>(bytes).is_ok_and(|v| v["hum"] == "reply"))
    })
}

pub struct Answer<'a> {
    pub to: &'a str,
    pub answers: &'a str,
    pub ok: bool,
    pub text: &'a [u8],
    pub lifetime: Duration,
}

pub fn reply(node_eid: &str, answer: &Answer, mtu: usize) -> Result<Vec<Vec<u8>>> {
    let source = EndpointID::try_from(node_eid).map_err(|e| anyhow::anyhow!("{node_eid}: {e:?}"))?;
    let dest = EndpointID::try_from(answer.to).map_err(|e| anyhow::anyhow!("{}: {e:?}", answer.to))?;
    let mark = serde_json::to_vec(&json!({"hum": "reply", "to": answer.answers, "ok": answer.ok}))?;
    let created = CreationTimestamp::now();
    let one = |fragment: Option<(u64, &[u8])>| -> Vec<u8> {
        let mut primary = PrimaryBlockBuilder::default()
            .destination(dest.clone())
            .source(source.clone())
            .report_to(EndpointID::none())
            .creation_timestamp(created.clone())
            .lifetime(answer.lifetime);
        let data = match fragment {
            Some((offset, data)) => {
                primary = primary
                    .bundle_control_flags(BundleControlFlags::BUNDLE_IS_FRAGMENT.bits())
                    .fragmentation_offset(offset)
                    .total_data_length(answer.text.len() as u64);
                data
            }
            None => answer.text,
        };
        let mut bundle = Bundle::new(
            primary.build().expect("a reply's primary block is whole"),
            vec![
                new_payload_block(BlockControlFlags::empty(), data.to_vec()),
                new_hop_count_block(2, BlockControlFlags::BLOCK_REPLICATE, HOP_LIMIT),
                mark_block(&mark),
            ],
        );
        bundle.set_crc(CRC_32);
        bundle.sort_canonicals();
        bundle.to_cbor()
    };
    let whole = one(None);
    if whole.len() <= mtu {
        return Ok(vec![whole]);
    }
    let overhead = one(Some((answer.text.len() as u64, &[]))).len() + MOST_HEADER;
    if mtu <= overhead {
        bail!("BP7_MTU {mtu} leaves no room for an answer beside its {overhead} bytes of blocks");
    }
    let chunk = mtu - overhead;
    Ok(answer
        .text
        .chunks(chunk)
        .enumerate()
        .map(|(i, data)| one(Some(((i * chunk) as u64, data))))
        .collect())
}

fn mark_block(mark: &[u8]) -> CanonicalBlock {
    CanonicalBlockBuilder::default()
        .block_type(HUM_BLOCK)
        .block_number(3)
        .block_control_flags(BlockControlFlags::BLOCK_REPLICATE.bits())
        .data(CanonicalData::Unknown(mark.to_vec()))
        .build()
        .expect("block 193 is whole")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "dtn://hum.test/inference";
    const YOU: &str = "dtn://you.test/";

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
    }

    fn answer(text: &[u8]) -> Answer<'_> {
        Answer { to: YOU, answers: "dtn://you.test/-1-0", ok: true, text, lifetime: Duration::from_secs(60) }
    }

    #[test]
    fn a_reply_is_heard_as_a_reply() {
        let bytes = reply(ME, &answer(b"hi"), 1232).unwrap().remove(0);
        let mut b = Bundle::try_from(bytes.as_slice()).unwrap();
        assert!(b.crc_valid() && b.validate().is_ok());
        b.primary.destination = EndpointID::try_from(ME).unwrap();
        b.calculate_crc();
        assert!(matches!(arrive(&b.to_cbor(), ME, now()), Arrival::Dropped("hum-reply")));
    }

    #[test]
    fn every_fragment_fits_and_they_cover_the_answer() {
        let text = vec![b'x'; 10_000];
        for mtu in [300, 1232, 4000] {
            let datagrams = reply(ME, &answer(&text), mtu).unwrap();
            assert!(datagrams.len() > 1);
            let mut covered = 0;
            for d in &datagrams {
                assert!(d.len() <= mtu, "{} > {mtu}", d.len());
                let b = Bundle::try_from(d.as_slice()).unwrap();
                assert_eq!(b.primary.fragmentation_offset as usize, covered);
                assert_eq!(b.primary.total_data_length as usize, text.len());
                covered += b.payload().unwrap().len();
            }
            assert_eq!(covered, text.len());
        }
    }

    #[test]
    fn an_mtu_too_small_for_the_blocks_is_an_error() {
        assert!(reply(ME, &answer(&[b'x'; 500]), 40).is_err());
    }

    #[test]
    fn a_clockless_ask_expires_by_its_age() {
        let primary = PrimaryBlockBuilder::default()
            .destination(EndpointID::try_from(ME).unwrap())
            .source(EndpointID::try_from(YOU).unwrap())
            .report_to(EndpointID::none())
            .creation_timestamp(CreationTimestamp::with_time_and_seq(0, 7))
            .lifetime(Duration::from_secs(10))
            .build()
            .unwrap();
        let with_age = |age_ms: Option<u64>| {
            let mut blocks = vec![new_payload_block(BlockControlFlags::empty(), b"q".to_vec())];
            if let Some(ms) = age_ms {
                blocks.push(bundle_protocol::canonical::new_bundle_age_block(2, BlockControlFlags::empty(), ms));
            }
            let mut b = Bundle::new(primary.clone(), blocks);
            b.set_crc(CRC_32);
            b.sort_canonicals();
            b.to_cbor()
        };
        let t = now();
        match arrive(&with_age(Some(4_000)), ME, t) {
            Arrival::Whole(ask, _) => assert_eq!(ask.expires, t + 6_000),
            other => panic!("{other:?}"),
        }
        assert!(matches!(arrive(&with_age(Some(11_000)), ME, t), Arrival::Dropped("expired")));
        assert!(
            matches!(arrive(&with_age(None), ME, t), Arrival::Dropped("invalid")),
            "RFC 9171 4.4.2: a bundle with no creation time carries its age"
        );
    }
}
