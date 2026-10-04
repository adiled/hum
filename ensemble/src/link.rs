use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{Hid, HumdAddr, PeerCapabilities, PeerConnection, Tone};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    pub drop_next: usize,
    pub dup_next: usize,
    pub reorder_next: usize,
    pub drop_every: usize,
    pub dup_every: usize,
}

impl Script {
    fn take(counted: &mut usize, every: usize, offered: u64) -> bool {
        match *counted {
            0 => every > 0 && offered % every as u64 == 0,
            n => {
                *counted = n - 1;
                true
            }
        }
    }

    fn verdict(&mut self, offered: u64) -> Option<Verdict> {
        let drop = Self::take(&mut self.drop_next, self.drop_every, offered);
        let duplicate = Self::take(&mut self.dup_next, self.dup_every, offered);
        (drop || duplicate).then_some(Verdict { drop, duplicate })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Noise {
    pub drop_pct: u8,
    pub dup_pct: u8,
}

impl Noise {
    fn verdict(&self, rng: &mut impl Rng) -> Verdict {
        Verdict {
            drop: self.drop_pct > 0 && rng.gen_range(0..100u8) < self.drop_pct,
            duplicate: self.dup_pct > 0 && rng.gen_range(0..100u8) < self.dup_pct,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub drop: bool,
    pub duplicate: bool,
}

#[derive(Debug, Clone)]
pub struct LinkFaults {
    pub script: Script,
    pub noise: Noise,
    seed: u64,
    offered: u64,
}

impl Default for LinkFaults {
    fn default() -> Self {
        Self { script: Script::default(), noise: Noise::default(), seed: 0x5EED_C0DE, offered: 0 }
    }
}

impl LinkFaults {
    pub fn script(mut self, script: Script) -> Self {
        self.script = script;
        self
    }

    pub fn noise(mut self, noise: Noise) -> Self {
        self.noise = noise;
        self
    }

    pub fn drop_next(mut self, n: usize) -> Self {
        self.script.drop_next = n;
        self
    }

    pub fn dup_next(mut self, n: usize) -> Self {
        self.script.dup_next = n;
        self
    }

    pub fn reorder_next(mut self, n: usize) -> Self {
        self.script.reorder_next = n;
        self
    }

    pub fn drop_pct(mut self, pct: u8, seed: u64) -> Self {
        self.noise.drop_pct = pct.min(100);
        self.seed = seed;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    fn verdict(&mut self, rng: &mut impl Rng) -> Verdict {
        self.offered += 1;
        self.script.verdict(self.offered).unwrap_or_else(|| self.noise.verdict(rng))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkCounters {
    pub offered: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub reordered: u64,
    pub buffered: u64,
    pub lost_on_heal: u64,
    pub evicted: u64,
    pub stalled_sends: u64,
}

impl LinkCounters {
    pub fn since(&self, base: &Self) -> Self {
        Self {
            offered: self.offered - base.offered,
            delivered: self.delivered - base.delivered,
            dropped: self.dropped - base.dropped,
            duplicated: self.duplicated - base.duplicated,
            reordered: self.reordered - base.reordered,
            buffered: self.buffered - base.buffered,
            lost_on_heal: self.lost_on_heal - base.lost_on_heal,
            evicted: self.evicted - base.evicted,
            stalled_sends: self.stalled_sends - base.stalled_sends,
        }
    }
}

pub const PARTITION_BUFFER_CAP: usize = 64;

pub struct InMemoryEndpoint {
    peer: HumdAddr,
    caps: PeerCapabilities,
    tx: Mutex<Option<mpsc::Sender<Tone>>>,
    rx: Mutex<Option<mpsc::Receiver<Tone>>>,
    partition: Mutex<PartitionState>,
    faults: Mutex<LinkFaults>,
    rng: Mutex<StdRng>,
    counters: Mutex<LinkCounters>,
    reorder_hold: Mutex<Option<Tone>>,
    killed: AtomicBool,
    stalled: AtomicBool,
}

struct PartitionState {
    partitioned: bool,
    buffer: VecDeque<Tone>,
}

impl InMemoryEndpoint {
    pub fn pair(
        a_id: Hid,
        a_caps: PeerCapabilities,
        b_id: Hid,
        b_caps: PeerCapabilities,
    ) -> (Arc<dyn PeerConnection>, Arc<dyn PeerConnection>) {
        let (a, b) = Self::pair_concrete(a_id, a_caps, b_id, b_caps);
        (a as Arc<dyn PeerConnection>, b as Arc<dyn PeerConnection>)
    }

    pub fn pair_concrete(
        a_id: Hid,
        a_caps: PeerCapabilities,
        b_id: Hid,
        b_caps: PeerCapabilities,
    ) -> (Arc<InMemoryEndpoint>, Arc<InMemoryEndpoint>) {
        let (tx_ab, rx_ab) = mpsc::channel::<Tone>(256);
        let (tx_ba, rx_ba) = mpsc::channel::<Tone>(256);
        let seed_a = LinkFaults::default().seed;
        let seed_b = seed_a ^ 0x9E37_79B9_7F4A_7C15;
        let a = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(b_id),
            caps: b_caps.clone(),
            tx: Mutex::new(Some(tx_ab)),
            rx: Mutex::new(Some(rx_ba)),
            partition: Mutex::new(PartitionState {
                partitioned: false,
                buffer: VecDeque::new(),
            }),
            faults: Mutex::new(LinkFaults { seed: seed_a, ..Default::default() }),
            rng: Mutex::new(StdRng::seed_from_u64(seed_a)),
            counters: Mutex::new(LinkCounters::default()),
            reorder_hold: Mutex::new(None),
            killed: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
        });
        let b = Arc::new(InMemoryEndpoint {
            peer: HumdAddr::new(a_id),
            caps: a_caps,
            tx: Mutex::new(Some(tx_ba)),
            rx: Mutex::new(Some(rx_ab)),
            partition: Mutex::new(PartitionState {
                partitioned: false,
                buffer: VecDeque::new(),
            }),
            faults: Mutex::new(LinkFaults { seed: seed_b, ..Default::default() }),
            rng: Mutex::new(StdRng::seed_from_u64(seed_b)),
            counters: Mutex::new(LinkCounters::default()),
            reorder_hold: Mutex::new(None),
            killed: AtomicBool::new(false),
            stalled: AtomicBool::new(false),
        });
        (a, b)
    }

    pub fn set_partitioned(&self, partitioned: bool) {
        let drained = {
            let mut p = self.partition.lock();
            p.partitioned = partitioned;
            match partitioned {
                true => Vec::new(),
                false => p.buffer.drain(..).collect(),
            }
        };
        for tone in drained {
            let verdict = self.take_verdict();
            let lost = verdict.drop || self.push(tone, verdict.duplicate).is_err();
            if lost {
                self.counters.lock().lost_on_heal += 1;
            }
        }
    }

    fn try_emit(&self, tone: Tone) -> Result<()> {
        let guard = self.tx.lock();
        let Some(tx) = guard.as_ref() else {
            return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short()));
        };
        tx.try_send(tone).map_err(|e| anyhow::anyhow!("push: {e}"))
    }

    async fn emit(&self, tone: Tone) -> Result<()> {
        if self.stalled.load(Ordering::SeqCst) {
            self.counters.lock().stalled_sends += 1;
            return Ok(std::future::pending::<()>().await);
        }
        let tx = {
            let mut guard = self.tx.lock();
            match guard.take() {
                Some(tx) => tx,
                None => return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short())),
            }
        };
        let sent = tx.send(tone).await;
        let mut guard = self.tx.lock();
        if guard.is_none() {
            *guard = Some(tx);
        }
        sent.map_err(|e| anyhow::anyhow!("send: {e}"))
    }

    fn push(&self, tone: Tone, duplicate: bool) -> Result<()> {
        self.try_emit(tone.clone())?;
        self.counters.lock().delivered += 1;
        if duplicate && self.try_emit(tone).is_ok() {
            let mut c = self.counters.lock();
            c.delivered += 1;
            c.duplicated += 1;
        }
        Ok(())
    }

    fn take_verdict(&self) -> Verdict {
        let mut rng = self.rng.lock();
        self.faults.lock().verdict(&mut *rng)
    }

    fn take_reorder(&self) -> bool {
        if self.reorder_hold.lock().is_some() {
            let mut f = self.faults.lock();
            f.script.reorder_next = f.script.reorder_next.saturating_sub(1);
            return true;
        }
        self.faults.lock().script.reorder_next > 0
    }

    pub fn set_faults(&self, faults: LinkFaults) {
        let seed = faults.seed;
        *self.faults.lock() = faults;
        *self.rng.lock() = StdRng::seed_from_u64(seed);
    }

    pub fn faults(&self) -> LinkFaults { self.faults.lock().clone() }

    pub fn counters(&self) -> LinkCounters { *self.counters.lock() }

    pub fn buffered(&self) -> usize { self.partition.lock().buffer.len() }

    pub fn is_partitioned(&self) -> bool { self.partition.lock().partitioned }

    pub fn flush_reorder(&self) -> bool {
        match self.reorder_hold.lock().take() {
            Some(tone) => self.try_emit(tone).is_ok(),
            None => false,
        }
    }

    pub fn stall(&self) {
        self.stalled.store(true, Ordering::SeqCst);
    }

    pub fn unstall(&self) {
        self.stalled.store(false, Ordering::SeqCst);
    }

    pub fn is_stalled(&self) -> bool {
        self.stalled.load(Ordering::SeqCst)
    }

    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        self.tx.lock().take();
    }

    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PeerConnection for InMemoryEndpoint {
    fn peer(&self) -> &HumdAddr { &self.peer }
    fn capabilities(&self) -> &PeerCapabilities { &self.caps }

    async fn send(&self, tone: Tone) -> Result<()> {
        if self.killed.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("link to {} is dead", self.peer.id.short()));
        }
        self.counters.lock().offered += 1;

        {
            let mut p = self.partition.lock();
            if p.partitioned {
                if p.buffer.len() >= PARTITION_BUFFER_CAP {
                    p.buffer.pop_front();
                    self.counters.lock().evicted += 1;
                }
                p.buffer.push_back(tone);
                self.counters.lock().buffered += 1;
                return Ok(());
            }
        }

        let verdict = self.take_verdict();
        if verdict.drop {
            self.counters.lock().dropped += 1;
            return Ok(());
        }

        if self.take_reorder() {
            let held = self.reorder_hold.lock().take();
            if let Some(held) = held {
                self.counters.lock().reordered += 1;
                self.emit(tone).await?;
                self.counters.lock().delivered += 1;
                self.emit(held).await?;
                self.counters.lock().delivered += 1;
                return Ok(());
            }
            *self.reorder_hold.lock() = Some(tone);
            return Ok(());
        }

        self.emit(tone.clone()).await?;
        self.counters.lock().delivered += 1;
        if verdict.duplicate {
            self.emit(tone).await?;
            let mut c = self.counters.lock();
            c.delivered += 1;
            c.duplicated += 1;
        }
        Ok(())
    }

    fn take_receiver(&self) -> Option<mpsc::Receiver<Tone>> {
        self.rx.lock().take()
    }

    fn close(&self) {
        self.tx.lock().take();
        let _ = self.rx.lock().take();
    }
}
