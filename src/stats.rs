//! Rolling per-endpoint statistics + latency sparkline.

use std::collections::VecDeque;
use std::time::Duration;

pub const HISTORY: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Up,
    Down,
    /// UDP only: no reply, no refusal — open or filtered.
    Open,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "···",
            Self::Up => "UP",
            Self::Down => "DOWN",
            Self::Open => "OPEN",
        }
    }
    pub fn is_up(self) -> bool {
        matches!(self, Self::Up | Self::Open)
    }
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub sent: u64,
    pub recv: u64,
    pub status: Status,
    pub last: Option<Duration>,
    pub min: Option<Duration>,
    pub max: Option<Duration>,
    sum_ms: f64,
    prev_ms: Option<f64>,
    jitter_ms: f64,
    pub last_error: Option<String>,
    pub last_response: Option<Vec<u8>>,
    pub responses: u64,
    pub history: VecDeque<Option<f64>>, // ms, None = lost
    pub seq: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            sent: 0,
            recv: 0,
            status: Status::Pending,
            last: None,
            min: None,
            max: None,
            sum_ms: 0.0,
            prev_ms: None,
            jitter_ms: 0.0,
            last_error: None,
            last_response: None,
            responses: 0,
            history: VecDeque::with_capacity(HISTORY),
            seq: 0,
        }
    }
}

impl Stats {
    pub fn record_ok(&mut self, status: Status, rtt: Duration, response: Option<Vec<u8>>) {
        self.sent += 1;
        self.recv += 1;
        self.seq += 1;
        self.status = status;
        if response.is_some() {
            self.responses += 1;
            self.last_response = response;
        }
        let ms = rtt.as_secs_f64() * 1000.0;
        self.sum_ms += ms;
        self.min = Some(self.min.map_or(rtt, |m: Duration| m.min(rtt)));
        self.max = Some(self.max.map_or(rtt, |m: Duration| m.max(rtt)));
        if let Some(prev) = self.prev_ms {
            // RFC 3550-style smoothed jitter.
            self.jitter_ms += ((ms - prev).abs() - self.jitter_ms) / 16.0;
        }
        self.prev_ms = Some(ms);
        self.push(Some(ms));
    }

    pub fn record_fail(&mut self, error: impl Into<String>) {
        self.sent += 1;
        self.seq += 1;
        self.status = Status::Down;
        self.last_error = Some(error.into());
        self.push(None);
    }

    fn push(&mut self, v: Option<f64>) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(v);
    }

    pub fn avg_ms(&self) -> Option<f64> {
        (self.recv > 0).then(|| self.sum_ms / self.recv as f64)
    }
    pub fn jitter_ms(&self) -> f64 {
        self.jitter_ms
    }
    pub fn loss_pct(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            100.0 * (self.sent - self.recv) as f64 / self.sent as f64
        }
    }
}

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Unicode block sparkline; lost probes render as `·` in dim position.
pub fn sparkline(history: &VecDeque<Option<f64>>, width: usize) -> String {
    let vals: Vec<&Option<f64>> = history.iter().rev().take(width).collect();
    let max = vals.iter().filter_map(|v| **v).fold(1.0f64, f64::max);
    let mut s = String::with_capacity(width);
    for v in vals.iter().rev() {
        match v {
            Some(ms) => {
                let idx = ((ms / max) * 7.0).round() as usize;
                s.push(BARS[idx.min(7)]);
            }
            None => s.push('·'),
        }
    }
    s
}

pub fn fmt_ms(d: Option<Duration>) -> String {
    match d {
        None => "—".into(),
        Some(d) => {
            let ms = d.as_secs_f64() * 1000.0;
            if ms < 10.0 {
                format!("{ms:.2}ms")
            } else if ms < 100.0 {
                format!("{ms:.1}ms")
            } else {
                format!("{ms:.0}ms")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_track_loss_and_avg() {
        let mut s = Stats::default();
        s.record_ok(Status::Up, Duration::from_millis(10), None);
        s.record_fail("timeout");
        s.record_ok(
            Status::Up,
            Duration::from_millis(30),
            Some(b"pong".to_vec()),
        );
        assert_eq!(s.sent, 3);
        assert_eq!(s.recv, 2);
        assert!((s.loss_pct() - 33.33).abs() < 0.1);
        assert_eq!(s.avg_ms(), Some(20.0));
    }

    #[test]
    fn sparkline_renders() {
        let mut s = Stats::default();
        s.record_ok(Status::Up, Duration::from_millis(5), None);
        s.record_fail("x");
        let line = sparkline(&s.history, 10);
        assert_eq!(line.chars().count(), 2);
        assert!(line.contains('·'));
    }
}
