//! Countries for the connection map, from geoip.dat (no network): IPs are
//! looked up offline; domains are resolved the way the rule tester does.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use prism_core::common::is_private_ip;
use prism_core::geo::{self, IpSet};

use crate::store::{CoreKind, Settings};

pub struct GeoDb {
    sets: Vec<(String, IpSet)>,
}

impl GeoDb {
    /// The country categories (two letters) of a geoip.dat.
    pub fn load(path: &Path) -> std::io::Result<GeoDb> {
        let codes: Vec<String> = geo::codes(path)?
            .into_iter()
            .filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_lowercase()))
            .collect();
        let mut sets: Vec<(String, IpSet)> = geo::load_geoip(path, &codes)?.into_iter().map(|(c, s)| (c.to_ascii_uppercase(), s)).collect();
        // Biggest first: most addresses are found early.
        sets.sort_by_key(|(_, s)| std::cmp::Reverse(s.len()));
        Ok(GeoDb { sets })
    }

    pub fn country(&self, ip: IpAddr) -> Option<&str> {
        if is_private_ip(ip) {
            return None;
        }
        self.sets.iter().find(|(_, s)| s.contains(ip)).map(|(c, _)| c.as_str())
    }
}

static DB: OnceLock<GeoDb> = OnceLock::new();
static RESOLVED: OnceLock<Mutex<HashMap<String, Option<IpAddr>>>> = OnceLock::new();

/// Country code (or None) for each host: IPs directly, domains after a lookup.
pub async fn locate(runtime: &Path, s: &Settings, running: Option<CoreKind>, hosts: Vec<String>) -> anyhow::Result<HashMap<String, Option<String>>> {
    if DB.get().is_none() {
        let path = runtime.join("geoip.dat");
        let db = tauri::async_runtime::spawn_blocking(move || GeoDb::load(&path))
            .await?
            .map_err(|e| anyhow::anyhow!("geoip.dat: {e}（可在设置页「更新 GeoSite / GeoIP」下载）"))?;
        let _ = DB.set(db);
    }
    let db = DB.get().unwrap();
    let cache = RESOLVED.get_or_init(Default::default);
    let lookups = hosts.into_iter().map(|h| async move {
        let ip = match h.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
            Ok(ip) => Some(ip),
            Err(_) => {
                let known = cache.lock().unwrap().get(&h).copied();
                match known {
                    Some(ip) => ip,
                    None => {
                        let ip = crate::ruletest::lookup(s, running, &h).await;
                        let mut c = cache.lock().unwrap();
                        if c.len() > 5000 {
                            c.clear();
                        }
                        c.insert(h.clone(), ip);
                        ip
                    }
                }
            }
        };
        (h, ip.and_then(|ip| db.country(ip).map(String::from)))
    });
    Ok(futures::future::join_all(lookups).await.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in: PRISM_GEOIP=path/to/geoip.dat
    #[test]
    fn looks_up_countries() {
        let Ok(path) = std::env::var("PRISM_GEOIP") else { return };
        let t = std::time::Instant::now();
        let db = GeoDb::load(Path::new(&path)).unwrap();
        eprintln!("loaded {} countries in {:?}", db.sets.len(), t.elapsed());
        assert_eq!(db.country("8.8.8.8".parse().unwrap()), Some("US"));
        assert_eq!(db.country("223.5.5.5".parse().unwrap()), Some("CN"));
        assert_eq!(db.country("1.1.1.1".parse().unwrap()).is_some(), true);
        assert_eq!(db.country("192.168.1.1".parse().unwrap()), None);
        let t = std::time::Instant::now();
        for i in 0..10000u32 {
            db.country(IpAddr::from((i * 7919).to_be_bytes()));
        }
        eprintln!("10k lookups in {:?}", t.elapsed());
    }
}
