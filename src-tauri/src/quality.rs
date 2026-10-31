//! 节点质量: latency samples per node over the last 24 hours (quality.json),
//! collected from the core's delay history (url-test groups, the user's
//! tests) plus a periodic test of the nodes in use.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;

/// Seconds of history kept.
const KEEP: i64 = 24 * 3600;
/// Samples kept per node (one a minute at most is stored).
const MAX_SAMPLES: usize = 1440;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct NodeStats {
    pub name: String,
    /// (unix seconds, milliseconds; 0 = failed)
    pub samples: Vec<(i64, u16)>,
    /// Mean delay of the successful tests.
    pub avg: Option<u32>,
    /// Mean change between consecutive successful tests.
    pub jitter: Option<u32>,
    /// Failed share, 0..=1.
    pub loss: f64,
    /// Lower is better: avg + 2 × jitter + 1000 × loss.
    pub score: Option<u32>,
}

pub struct Quality {
    path: PathBuf,
    nodes: Mutex<HashMap<String, VecDeque<(i64, u16)>>>,
}

impl Quality {
    pub fn load(path: PathBuf) -> Self {
        let nodes = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Quality { path, nodes: Mutex::new(nodes) }
    }

    pub fn save(&self) {
        let nodes = self.nodes.lock().unwrap().clone();
        let tmp = self.path.with_extension("tmp");
        if serde_json::to_vec(&nodes).ok().and_then(|b| std::fs::write(&tmp, b).ok()).is_some() {
            let _ = std::fs::rename(tmp, &self.path);
        }
    }

    /// Adds a sample unless one at or after `t` is already there.
    pub fn record(&self, name: &str, t: i64, delay: u16, now: i64) {
        let mut nodes = self.nodes.lock().unwrap();
        let h = nodes.entry(name.to_string()).or_default();
        if h.back().is_some_and(|(last, _)| *last >= t) {
            return;
        }
        h.push_back((t, delay));
        while h.len() > MAX_SAMPLES || h.front().is_some_and(|(ft, _)| *ft < now - KEEP) {
            h.pop_front();
        }
    }

    /// Takes the delay histories out of a `/proxies` answer (nodes only).
    pub fn ingest(&self, proxies: &Value, now: i64) -> usize {
        let mut n = 0;
        let Some(map) = proxies["proxies"].as_object() else { return 0 };
        for (name, p) in map {
            if p.get("all").is_some() || matches!(p["type"].as_str(), Some("Direct" | "Reject" | "RejectDrop" | "Pass" | "Compatible")) {
                continue;
            }
            for h in p["history"].as_array().into_iter().flatten() {
                let Some(t) = h["time"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) else { continue };
                let t = t.timestamp();
                if t < now - KEEP {
                    continue;
                }
                let d = h["delay"].as_u64().unwrap_or(0).min(u16::MAX as u64) as u16;
                self.record(name, t, d, now);
                n += 1;
            }
        }
        n
    }

    pub fn stats(&self, now: i64) -> Vec<NodeStats> {
        let nodes = self.nodes.lock().unwrap();
        let mut out: Vec<NodeStats> = nodes
            .iter()
            .map(|(name, h)| stats_of(name, h.iter().copied().filter(|(t, _)| *t >= now - KEEP).collect()))
            .filter(|s| !s.samples.is_empty())
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

fn stats_of(name: &str, samples: Vec<(i64, u16)>) -> NodeStats {
    let ok: Vec<u32> = samples.iter().filter(|(_, d)| *d > 0).map(|(_, d)| *d as u32).collect();
    let loss = if samples.is_empty() { 0.0 } else { (samples.len() - ok.len()) as f64 / samples.len() as f64 };
    let avg = (!ok.is_empty()).then(|| ok.iter().sum::<u32>() / ok.len() as u32);
    let jitter = (ok.len() > 1).then(|| ok.windows(2).map(|w| w[0].abs_diff(w[1])).sum::<u32>() / (ok.len() as u32 - 1));
    let score = avg.map(|a| a + 2 * jitter.unwrap_or(0) + (loss * 1000.0) as u32);
    NodeStats { name: name.to_string(), samples, avg, jitter, loss, score }
}

/// The node each group ends at, following selections: the nodes in use.
pub fn selected_nodes(proxies: &Value) -> Vec<String> {
    let Some(map) = proxies["proxies"].as_object() else { return Vec::new() };
    let mut out = Vec::new();
    for p in map.values().filter(|p| p.get("all").is_some()) {
        let mut cur = p["now"].as_str().unwrap_or_default().to_string();
        for _ in 0..10 {
            match map.get(&cur) {
                Some(next) if next.get("all").is_some() => cur = next["now"].as_str().unwrap_or_default().to_string(),
                _ => break,
            }
        }
        let is_node = map.get(&cur).is_some_and(|n| n.get("all").is_none() && !matches!(n["type"].as_str(), Some("Direct" | "Reject" | "RejectDrop" | "Pass")));
        if is_node && !out.contains(&cur) {
            out.push(cur);
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ingests_histories_and_scores() {
        let q = Quality::load(std::env::temp_dir().join("prism-quality-test-none.json"));
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-01T12:00:00Z").unwrap().timestamp();
        let proxies = json!({ "proxies": {
            "PROXY": { "type": "Selector", "all": ["A", "B"], "now": "AUTO" },
            "AUTO": { "type": "URLTest", "all": ["A", "B"], "now": "B", "history": [] },
            "A": { "type": "Shadowsocks", "history": [
                { "time": "2026-11-01T11:00:00Z", "delay": 100 },
                { "time": "2026-11-01T11:05:00Z", "delay": 140 },
                { "time": "2026-11-01T11:10:00Z", "delay": 0 },
                { "time": "2026-10-30T11:10:00Z", "delay": 50 }
            ]},
            "B": { "type": "Trojan", "history": [{ "time": "2026-11-01T11:00:00Z", "delay": 80 }] },
            "DIRECT": { "type": "Direct", "history": [{ "time": "2026-11-01T11:00:00Z", "delay": 1 }] }
        }});
        assert_eq!(q.ingest(&proxies, now), 4, "the 2-day-old sample and DIRECT are skipped");
        // The same history again adds nothing.
        q.ingest(&proxies, now);
        let s = q.stats(now);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        let a = &s[0];
        assert_eq!(a.samples.len(), 3);
        assert_eq!((a.avg, a.jitter), (Some(120), Some(40)));
        assert!((a.loss - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(a.score, Some(120 + 80 + 333));
        assert_eq!(selected_nodes(&proxies), ["B"]);
    }
}
