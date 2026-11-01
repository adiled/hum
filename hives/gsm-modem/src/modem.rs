//! The modem side: AT commands over the one serial port, read by one task
//! and written by whichever text is replying.
//!
//! A text arrives as a `+CMT: "<number>",...` line and its body, which
//! runs to the CR LF the modem ends every line with; a bare LF in it is
//! the text's own line break, and stays. A reply goes out as
//! `AT+CMGS="<number>"`, a pause, the body and Ctrl-Z, with the port held
//! throughout so no other reply lands inside it.
//!
//! Nothing is flushed: a serial port's flush is tcdrain, which blocks its
//! thread until the wire takes the bytes, and a port that stalls would
//! stall every task on that thread with it. A write already hands the
//! bytes to the kernel.

use std::io;
use std::time::Duration;

use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::warn;

use crate::gsm;

pub const INIT: [&str; 4] = ["AT", "AT+CMGF=1", "AT+CSCS=\"GSM\"", "AT+CNMI=2,2,0,0,0"];
const CTRL_Z: u8 = 0x1A;

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub from: String,
    pub body: String,
}

struct Lines;

impl Decoder for Lines {
    type Item = Vec<u8>;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<Vec<u8>>> {
        let Some(at) = src.windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let line = src.split_to(at + 2);
        Ok(Some(line[..at].to_vec()))
    }
}

/// Reads the texts the modem pushes until the port closes.
pub async fn read<R: AsyncRead + Unpin>(port: R, mut text: impl FnMut(Text)) {
    let mut lines = FramedRead::new(port, Lines);
    let mut from: Option<String> = None;
    while let Some(line) = lines.next().await {
        let line = match line {
            Ok(line) => gsm::decode(&line),
            Err(e) => {
                warn!(error = %e, "serial.read");
                continue;
            }
        };
        if let Some(header) = line.strip_prefix("+CMT:") {
            from = header.split('"').nth(1).map(str::to_string);
            continue;
        }
        if let Some(from) = from.take()
            && !line.is_empty()
        {
            text(Text { from, body: line });
        }
    }
}

pub struct Modem<W> {
    port: Mutex<W>,
}

impl<W: AsyncWrite + Unpin> Modem<W> {
    pub fn new(port: W) -> Self {
        Self { port: Mutex::new(port) }
    }

    pub async fn init(&self) -> io::Result<()> {
        for cmd in INIT {
            self.port.lock().await.write_all(&[cmd.as_bytes(), b"\r"].concat()).await?;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Ok(())
    }

    pub async fn send(&self, to: &str, body: &[u8]) -> io::Result<()> {
        let mut port = self.port.lock().await;
        port.write_all(&[b"AT+CMGS=\"", gsm::encode(to).as_slice(), b"\"\r"].concat()).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        port.write_all(&[body, &[CTRL_Z]].concat()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn texts(wire: &[u8]) -> Vec<Text> {
        let mut got = Vec::new();
        read(wire, |t| got.push(t)).await;
        got
    }

    #[tokio::test]
    async fn a_text_is_its_whole_body() {
        let wire = b"\r\n+CMT: \"+14155551234\",\"\",\"25/05/17,12:00:00+00\"\r\nline one\nline two\n\nthree\r\n";
        assert_eq!(
            texts(wire).await,
            [Text { from: "+14155551234".into(), body: "line one\nline two\n\nthree".into() }]
        );
    }

    #[tokio::test]
    async fn texts_back_to_back_stay_apart() {
        let wire = b"+CMT: \"+1\",,\"25/05/17,12:00:00+00\"\r\none\r\n\r\n+CMT: \"+2\",,\"25/05/17,12:00:01+00\"\r\ntwo\r\nOK\r\nRING\r\n";
        assert_eq!(
            texts(wire).await,
            [Text { from: "+1".into(), body: "one".into() }, Text { from: "+2".into(), body: "two".into() }]
        );
    }

    #[tokio::test]
    async fn a_body_is_read_in_the_gsm_charset() {
        let wire = b"+CMT: \"+1\",,\"x\"\r\n\x00home \x02 5 \x1b\x65\r\n";
        assert_eq!(texts(wire).await[0].body, "@home $ 5 €");
    }

    #[tokio::test]
    async fn a_reply_is_one_command_with_nothing_inside_it() {
        let (here, mut there) = tokio::io::duplex(4096);
        let modem = std::sync::Arc::new(Modem::new(here));
        let (a, b) = (modem.clone(), modem.clone());
        let one = tokio::spawn(async move { a.send("+1", b"first").await });
        let two = tokio::spawn(async move { b.send("+2", b"second").await });
        one.await.unwrap().unwrap();
        two.await.unwrap().unwrap();
        drop(modem);
        let mut wrote = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut there, &mut wrote).await.unwrap();
        let sends: Vec<&[u8]> = wrote.split(|b| *b == CTRL_Z).filter(|s| !s.is_empty()).collect();
        let mut sends: Vec<String> = sends.iter().map(|s| String::from_utf8_lossy(s).into_owned()).collect();
        sends.sort();
        assert_eq!(sends, ["AT+CMGS=\"+1\"\rfirst", "AT+CMGS=\"+2\"\rsecond"]);
    }
}
