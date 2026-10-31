//! 带宽测速: download speed through each node, measured in this process with
//! prism-core's outbounds (so it works whichever core is running). In TUN
//! mode the sockets are pinned to the physical adapter and names resolved
//! with the profile's DNS, like the core does for its own connections.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Result};
use prism_core::delay::{self, Speed};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::store::{CoreKind, Settings, Store};

static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Clone)]
pub struct Progress {
    pub name: String,
    pub speed: Option<Speed>,
    pub error: Option<String>,
    pub done: bool,
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        prism_core::net::set_resolver(None);
        prism_core::net::set_bind_interface(0);
        BUSY.store(false, Ordering::SeqCst);
    }
}

pub async fn run(app: &AppHandle, store: &Store, s: &Settings, running: Option<CoreKind>, names: Vec<String>, url: &str, secs: u64) -> Result<Vec<Progress>> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err(anyhow!("正在测速，请等待完成"));
    }
    let _reset = Reset;
    let tun = s.tun && running.is_some();
    let mut profile = crate::core::rule_test_profile(store, s)?;
    if tun && !profile.dns.enable {
        // The system resolver answers with fake-ips while TUN is on.
        profile.dns.enable = true;
        profile.dns.enhanced_mode = "normal".into();
        profile.dns.nameserver = vec!["223.5.5.5".into(), "119.29.29.29".into()];
    }
    profile.dns.listen = None;
    let core = prism_core::build(profile, &store.runtime_dir(), &prism_core::BuildEnv::standalone()).await?;
    let stop = core.shutdown.clone();
    let _stop = scopeguard(move || stop.cancel());
    if tun {
        let index = crate::lan::physical_index().ok_or_else(|| anyhow!("找不到物理网卡，TUN 模式下无法测速"))?;
        prism_core::net::set_bind_interface(index);
    }
    prism_core::net::set_resolver(core.dns.clone());

    let reg = core.tunnel.reg.clone();
    let mut out = Vec::new();
    for name in names {
        let p = if reg.get(&name).is_none() {
            Progress { name: name.clone(), speed: None, error: Some("prism-core 不支持这个节点的协议，无法测速".into()), done: true }
        } else {
            let emit = |sp: &Speed| {
                let _ = app.emit("speed-progress", Progress { name: name.clone(), speed: Some(sp.clone()), error: None, done: false });
            };
            match delay::speed_test(&reg, &name, url, Duration::from_secs(secs.clamp(3, 30)), emit).await {
                Ok(sp) => Progress { name: name.clone(), speed: Some(sp), error: None, done: true },
                Err(e) => Progress { name: name.clone(), speed: None, error: Some(e.to_string()), done: true },
            }
        };
        let _ = app.emit("speed-progress", p.clone());
        out.push(p);
    }
    Ok(out)
}

/// Runs `f` when dropped.
fn scopeguard(f: impl FnOnce()) -> impl Drop {
    struct G<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for G<F> {
        fn drop(&mut self) {
            if let Some(f) = self.0.take() {
                f();
            }
        }
    }
    G(Some(f))
}
