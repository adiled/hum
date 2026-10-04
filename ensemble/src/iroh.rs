
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, PublicKey, TransportAddr};

use crate::{HumdAddr, Hid, PeerCapabilities, PeerConnection, Tone, Transport};

pub const IROH_ALPN: &[u8] = b"hum/0.5";

const RECV_CAP: usize = 256;

pub const IROH_HINT: &str = "iroh:";

pub const IROH_IP_HINT: &str = "iroh-ip:";

const PRIMING_NEWLINE: &[u8] = b"\n";

pub fn dialable_addr(bound: std::net::SocketAddr) -> std::net::SocketAddr {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    let ip = match bound.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    };
    SocketAddr::new(ip, bound.port())
}

pub fn humd_id_from_node_id(node_id: &EndpointId) -> Hid {
    let mut h = Sha256::new();
    h.update(node_id.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest[..32]);
    Hid::from(out)
}

pub struct IrohEndpoint {
    peer: HumdAddr,
    node_id: EndpointId,
    caps: PeerCapabilities,
    _connection: Connection,
    sender: Mutex<Option<SendStream>>,
    rx: parking_lot::Mutex<Option<mpsc::Receiver<Tone>>>,
}

impl IrohEndpoint {
    pub fn from_streams(
        node_id: EndpointId,
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        caps: PeerCapabilities,
    ) -> Arc<Self> {
        let humd_id = humd_id_from_node_id(&node_id);
        let peer = HumdAddr::new(humd_id).with_hint(format!("{IROH_HINT}{}", hex::encode(node_id.as_bytes())));
        let (tx, rx) = mpsc::channel::<Tone>(RECV_CAP);
        let me = Arc::new(Self {
            peer,
            node_id,
            caps,
            _connection: conn,
            sender: Mutex::new(Some(send)),
            rx: parking_lot::Mutex::new(Some(rx)),
        });
        tokio::spawn(read_loop(recv, tx));
        me
    }

    pub async fn connect(
        endpoint: &Endpoint,
        addr: EndpointAddr,
        caps: PeerCapabilities,
    ) -> Result<Arc<Self>> {
        let node_id = addr.id;
        let conn = endpoint
            .connect(addr, IROH_ALPN)
            .await
            .map_err(|e| anyhow!("iroh connect: {e}"))?;
        let (mut send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| anyhow!("iroh open_bi: {e}"))?;
        send.write_all(PRIMING_NEWLINE)
            .await
            .map_err(|e| anyhow!("iroh prime stream: {e}"))?;
        Ok(Self::from_streams(node_id, conn, send, recv, caps))
    }

    pub fn node_id(&self) -> &EndpointId {
        &self.node_id
    }
}

#[async_trait]
impl PeerConnection for IrohEndpoint {
    fn peer(&self) -> &HumdAddr {
        &self.peer
    }
    fn capabilities(&self) -> &PeerCapabilities {
        &self.caps
    }

    async fn send(&self, tone: Tone) -> Result<()> {
        let mut line = serde_json::to_vec(&tone)
            .map_err(|e| anyhow!("iroh send: serialize: {e}"))?;
        line.push(b'\n');
        let mut guard = self.sender.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| anyhow!("iroh send: stream closed"))?;
        s.write_all(&line)
            .await
            .map_err(|e| anyhow!("iroh send: write: {e}"))?;
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        let slot = match self.sender.try_lock() {
            Ok(mut g) => g.take(),
            Err(_) => None,
        };
        if let Some(mut s) = slot {
            tokio::spawn(async move {
                let _ = s.finish();
            });
        }
        let _ = self.rx.lock().take();
    }
}

async fn read_loop(recv: RecvStream, tx: mpsc::Sender<Tone>) {
    let mut lines = BufReader::new(recv).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                tracing::trace!(target: "ensemble.iroh", err = %e, "iroh.read.failed");
                break;
            }
        };
        if line.is_empty() {
            continue;
        }
        let tone: Tone = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!(target: "ensemble.iroh", err = %e, "iroh.parse.failed");
                continue;
            }
        };
        if tx.send(tone).await.is_err() {
            break;
        }
    }
}

pub struct IrohTransport {
    endpoint: Endpoint,
}

impl IrohTransport {
    pub fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub async fn bind_direct() -> Result<Self> {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind: {e}"))?;
        Ok(Self::new(endpoint))
    }

    pub async fn bind_direct_with_key(humd_key: &crate::HumdKey) -> Result<Self> {
        let sk = iroh::SecretKey::from_bytes(&humd_key.0.to_bytes());
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .relay_mode(iroh::RelayMode::Disabled)
            .secret_key(sk)
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind (with key): {e}"))?;
        Ok(Self::new(endpoint))
    }

    pub async fn bind_relayed() -> Result<Self> {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![IROH_ALPN.to_vec()])
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind (relayed): {e}"))?;
        Ok(Self::new(endpoint))
    }

    pub async fn bind_with_relay(relay_url: iroh::RelayUrl) -> Result<Self> {
        let map = iroh::RelayMap::from(relay_url);
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .relay_mode(iroh::RelayMode::Custom(map))
            .bind()
            .await
            .map_err(|e| anyhow!("iroh bind (custom relay): {e}"))?;
        Ok(Self::new(endpoint))
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn dial_hints(&self) -> Vec<String> {
        self.endpoint
            .bound_sockets()
            .into_iter()
            .map(|s| format!("{IROH_IP_HINT}{}", dialable_addr(s)))
            .collect()
    }

    pub async fn accept(&self) -> Result<Arc<IrohEndpoint>> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| anyhow!("iroh accept: endpoint closed"))?;
        let conn = incoming
            .await
            .map_err(|e| anyhow!("iroh accept: handshake: {e}"))?;
        let node_id = conn.remote_id();
        let (send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| anyhow!("iroh accept: accept_bi: {e}"))?;
        let mut priming_newline = [0u8; PRIMING_NEWLINE.len()];
        let _ = recv.read(&mut priming_newline).await;
        Ok(IrohEndpoint::from_streams(
            node_id,
            conn,
            send,
            recv,
            PeerCapabilities::default(),
        ))
    }
}

#[async_trait]
impl Transport for IrohTransport {
    async fn connect(&self, addr: &HumdAddr) -> Result<Arc<dyn PeerConnection>> {
        let hint = addr
            .hints
            .iter()
            .find_map(|h| h.strip_prefix(IROH_HINT))
            .ok_or_else(|| anyhow!("iroh transport: no `iroh:` hint in HumdAddr"))?;
        let node_id_bytes = hex::decode(hint)
            .with_context(|| format!("iroh transport: hint `{hint}` is not hex"))?;
        if node_id_bytes.len() != 32 {
            return Err(anyhow!("iroh transport: NodeId must be 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&node_id_bytes);
        let node_id = PublicKey::from_bytes(&arr)
            .map_err(|e| anyhow!("iroh transport: invalid NodeId: {e}"))?;
        let direct_addrs: Vec<TransportAddr> = addr
            .hints
            .iter()
            .filter_map(|h| h.strip_prefix(IROH_IP_HINT))
            .filter_map(|s| s.parse().ok())
            .map(TransportAddr::Ip)
            .collect();
        let endpoint_addr = EndpointAddr::from_parts(node_id, direct_addrs);
        let endpoint = IrohEndpoint::connect(
            &self.endpoint,
            endpoint_addr,
            PeerCapabilities::default(),
        )
        .await?;
        Ok(endpoint as Arc<dyn PeerConnection>)
    }
}

#[cfg(test)]
mod dial_tests {
    use super::dialable_addr;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn a_wildcard_bind_becomes_loopback() {
        let v4: SocketAddr = "0.0.0.0:41678".parse().expect("parse");
        assert_eq!(dialable_addr(v4), "127.0.0.1:41678".parse().expect("parse"));
        let v6: SocketAddr = "[::]:41678".parse().expect("parse");
        assert_eq!(dialable_addr(v6), "[::1]:41678".parse().expect("parse"));
    }

    #[test]
    fn the_port_is_preserved() {
        let a: SocketAddr = "0.0.0.0:1".parse().expect("parse");
        let b: SocketAddr = "0.0.0.0:65535".parse().expect("parse");
        assert_eq!(dialable_addr(a).port(), 1);
        assert_eq!(dialable_addr(b).port(), 65535);
    }

    #[test]
    fn a_concrete_address_is_left_alone() {
        for s in ["192.168.1.5:9000", "10.0.0.7:443", "[fe80::1]:9000", "127.0.0.1:5000"] {
            let before: SocketAddr = s.parse().expect("parse");
            assert_eq!(dialable_addr(before), before, "{s} must be untouched");
        }
    }

    #[test]
    fn normalising_twice_is_stable() {
        let v4: SocketAddr = "0.0.0.0:7000".parse().expect("parse");
        let once = dialable_addr(v4);
        assert_eq!(dialable_addr(once), once);
        assert!(!once.ip().is_unspecified());
        assert_ne!(once.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_ne!(dialable_addr(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 1)).ip(), IpAddr::V6(Ipv6Addr::UNSPECIFIED));
    }
}
