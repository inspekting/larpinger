//! Application state: probe scheduling, result collection, key handling.

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crate::payload::PayloadCfg;
use crate::probe::{probe, Outcome};
use crate::stats::Stats;
use crate::target::Endpoint;

pub struct App {
    pub endpoints: Vec<Endpoint>,
    pub stats: Vec<Stats>,
    pub selected: usize,
    pub paused: bool,
    pub interval: Duration,
    pub timeout: Duration,
    pub payload: PayloadCfg,
    pub expanded: bool,
    pub started: Instant,
    pub round: u64,
    pub count: u64, // 0 = continuous (default)
    pub pending: usize,
    pub finished: bool,
    tx: Sender<(usize, Outcome)>,
    pub rx: Receiver<(usize, Outcome)>,
    next_launch: Instant,
    pub last_activity: Instant,
}

impl App {
    pub fn new(
        endpoints: Vec<Endpoint>,
        interval: Duration,
        timeout: Duration,
        count: u64,
        payload: PayloadCfg,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let n = endpoints.len();
        Self {
            endpoints,
            stats: (0..n).map(|_| Stats::default()).collect(),
            selected: 0,
            paused: false,
            interval,
            timeout,
            payload,
            expanded: false,
            started: Instant::now(),
            round: 0,
            count,
            pending: 0,
            finished: false,
            tx,
            rx,
            next_launch: Instant::now(),
            last_activity: Instant::now(),
        }
    }

    /// Launch one probe per endpoint on its own thread (bounded by timeout).
    pub fn launch_round(&mut self) {
        if self.paused || self.finished {
            return;
        }
        if self.count > 0 && self.round >= self.count {
            if self.pending == 0 {
                self.finished = true;
            }
            return;
        }
        self.round += 1;
        let seq = (self.round & 0xFFFF) as u16;
        for (idx, ep) in self.endpoints.iter().enumerate() {
            let tx = self.tx.clone();
            let ep = ep.clone();
            let timeout = self.timeout;
            let cfg = self.payload.clone();
            self.pending += 1;
            std::thread::spawn(move || {
                let out = probe(&ep, timeout, seq, &cfg);
                let _ = tx.send((idx, out));
            });
        }
        self.next_launch = Instant::now() + self.interval;
    }

    /// Drain finished probes into stats. Returns labels for plain-mode logging.
    pub fn drain(&mut self) -> Vec<(usize, Outcome)> {
        let mut fresh = Vec::new();
        while let Ok((idx, out)) = self.rx.try_recv() {
            self.pending = self.pending.saturating_sub(1);
            self.last_activity = Instant::now();
            match &out {
                Outcome::Ok {
                    status,
                    rtt,
                    response,
                } => self.stats[idx].record_ok(*status, *rtt, response.clone()),
                Outcome::Fail(e) => self.stats[idx].record_fail(e.clone()),
            }
            fresh.push((idx, out));
        }
        if self.count > 0 && self.round >= self.count && self.pending == 0 && self.round > 0 {
            self.finished = true;
        }
        fresh
    }

    pub fn due(&self) -> bool {
        !self.paused && !self.finished && Instant::now() >= self.next_launch
    }

    pub fn reset_stats(&mut self) {
        for s in &mut self.stats {
            *s = Stats::default();
        }
        self.round = 0;
        self.started = Instant::now();
        self.finished = false;
        self.next_launch = Instant::now();
    }

    pub fn up_count(&self) -> usize {
        self.stats.iter().filter(|s| s.status.is_up()).count()
    }
    pub fn down_count(&self) -> usize {
        self.stats
            .iter()
            .filter(|s| s.status == crate::stats::Status::Down)
            .count()
    }
    pub fn total_sent(&self) -> u64 {
        self.stats.iter().map(|s| s.sent).sum()
    }
    pub fn total_recv(&self) -> u64 {
        self.stats.iter().map(|s| s.recv).sum()
    }
    pub fn global_loss(&self) -> f64 {
        let (sent, recv) = (self.total_sent(), self.total_recv());
        if sent == 0 {
            0.0
        } else {
            100.0 * (sent - recv) as f64 / sent as f64
        }
    }
    pub fn global_avg_ms(&self) -> Option<f64> {
        let vals: Vec<f64> = self.stats.iter().filter_map(|s| s.avg_ms()).collect();
        (!vals.is_empty()).then(|| vals.iter().sum::<f64>() / vals.len() as f64)
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.endpoints.is_empty() {
            return;
        }
        let len = self.endpoints.len() as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(len) as usize;
    }
}
