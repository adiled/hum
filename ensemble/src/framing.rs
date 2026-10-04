use anyhow::{Result, anyhow};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::Tone;

pub const MAX_FRAME_BYTES: usize = 256 * 1024;

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, tone: &Tone) -> Result<()> {
    let mut line = serde_json::to_vec(tone).map_err(|e| anyhow!("encode: {e}"))?;
    line.push(b'\n');
    writer
        .write_all(&line)
        .await
        .map_err(|e| anyhow!("write: {e}"))
}

pub async fn pump<R>(reader: R, tx: mpsc::Sender<Tone>, transport: &'static str)
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(reader);
    let mut line: Vec<u8> = Vec::new();
    let mut oversize = false;
    let mut discarded = 0u64;

    loop {
        let chunk = match reader.fill_buf().await {
            Ok(c) => c,
            Err(e) => {
                tracing::trace!(target: "ensemble.frame", transport, error = %e, "read.failed");
                break;
            }
        };
        if chunk.is_empty() {
            if !line.is_empty() && !oversize {
                forward(&line, &tx, transport).await;
            }
            break;
        }
        let newline = chunk.iter().position(|b| *b == b'\n');
        let take = newline.unwrap_or(chunk.len());
        let consumed = take + usize::from(newline.is_some());

        if line.len() + take > MAX_FRAME_BYTES {
            if !oversize {
                oversize = true;
                tracing::warn!(
                    target: "ensemble.frame",
                    transport,
                    cap = MAX_FRAME_BYTES,
                    "frame.oversize",
                );
            }
        } else {
            line.extend_from_slice(&chunk[..take]);
        }
        reader.consume(consumed);

        if newline.is_none() {
            continue;
        }
        if oversize {
            oversize = false;
            discarded += 1;
            line.clear();
            continue;
        }
        if !forward(&line, &tx, transport).await {
            break;
        }
        line.clear();
    }

    if discarded > 0 {
        tracing::warn!(target: "ensemble.frame", transport, discarded, "frame.oversize.total");
    }
}

async fn forward(line: &[u8], tx: &mpsc::Sender<Tone>, transport: &'static str) -> bool {
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        return true;
    }
    match serde_json::from_slice::<Tone>(line) {
        Ok(tone) => tx.send(tone).await.is_ok(),
        Err(e) => {
            tracing::trace!(target: "ensemble.frame", transport, error = %e, "frame.rejected");
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn frames(input: &[u8]) -> Vec<Tone> {
        let (tx, mut rx) = mpsc::channel(64);
        let (mut w, r) = tokio::io::duplex(input.len() + 64);
        w.write_all(input).await.unwrap();
        w.shutdown().await.unwrap();
        pump(r, tx, "test").await;
        let mut out = Vec::new();
        while let Ok(t) = rx.try_recv() {
            out.push(t);
        }
        out
    }

    #[tokio::test]
    async fn a_frame_survives_arriving_in_pieces() {
        let got = frames(b"{\"chi\":\"a\"}\n{\"chi\":\"b\"}\n").await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["chi"], "a");
        assert_eq!(got[1]["chi"], "b");
    }

    #[tokio::test]
    async fn one_byte_at_a_time_reassembles() {
        let raw = b"{\"chi\":\"drip\"}\n";
        let (tx, mut rx) = mpsc::channel(4);
        let (mut w, r) = tokio::io::duplex(64);
        let feeder = tokio::spawn(async move {
            for b in raw {
                w.write_all(&[*b]).await.unwrap();
                tokio::task::yield_now().await;
            }
            w.shutdown().await.unwrap();
        });
        pump(r, tx, "test").await;
        feeder.await.unwrap();
        let tone = rx.try_recv().unwrap();
        assert_eq!(tone["chi"], "drip");
    }

    #[tokio::test]
    async fn a_trailing_frame_without_a_newline_still_arrives() {
        let got = frames(b"{\"chi\":\"a\"}\n{\"chi\":\"tail\"}").await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[1]["chi"], "tail");
    }

    #[tokio::test]
    async fn garbage_is_skipped_and_the_next_frame_still_arrives() {
        let got = frames(b"not json\n{\"chi\":\"ok\"}\n").await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["chi"], "ok");
    }

    #[tokio::test]
    async fn a_blank_line_is_a_keepalive_not_a_tone() {
        let got = frames(b"\n\n{\"chi\":\"ok\"}\n").await;
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn an_oversize_frame_is_discarded_and_the_link_resyncs() {
        let mut input = vec![b'x'; MAX_FRAME_BYTES + 4096];
        input.push(b'\n');
        input.extend_from_slice(b"{\"chi\":\"after\"}\n");
        let got = frames(&input).await;
        assert_eq!(got.len(), 1, "the oversize frame must not be parsed");
        assert_eq!(got[0]["chi"], "after");
    }

    #[tokio::test]
    async fn a_frame_at_exactly_the_cap_is_accepted() {
        let mut input = br#"{"chi":""#.to_vec();
        input.extend(std::iter::repeat_n(b'p', MAX_FRAME_BYTES - input.len() - 2));
        input.extend_from_slice(br#""}"#);
        assert_eq!(input.len(), MAX_FRAME_BYTES);
        input.push(b'\n');
        let got = frames(&input).await;
        assert_eq!(got.len(), 1);
    }

    #[tokio::test]
    async fn pump_stops_when_the_receiver_is_gone() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let (mut w, r) = tokio::io::duplex(64);
        w.write_all(b"{\"chi\":\"a\"}\n{\"chi\":\"b\"}\n")
            .await
            .unwrap();
        pump(r, tx, "test").await;
    }

    #[tokio::test]
    async fn write_frame_emits_exactly_one_newline_terminated_line() {
        let (mut w, r) = tokio::io::duplex(256);
        let mut r = BufReader::new(r);
        write_frame(&mut w, &json!({"chi": "x", "n": 1}))
            .await
            .unwrap();
        drop(w);
        let mut first = String::new();
        r.read_line(&mut first).await.unwrap();
        assert_eq!(first, "{\"chi\":\"x\",\"n\":1}\n");
        let mut rest = String::new();
        assert_eq!(r.read_line(&mut rest).await.unwrap(), 0);
    }
}
