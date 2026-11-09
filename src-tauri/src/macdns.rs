//! macOS TUN DNS: the system resolver usually asks the router, which sits
//! on the local network and so outside the tunnel's routes. While TUN runs,
//! every enabled network service gets public DNS servers instead; those
//! are routed into the tunnel, where the core answers them (fake-ip, rules
//! ...). Should Prism die without restoring, the public servers still work.
//!
//! The previous settings are kept in `dns-backup.json` in the data folder
//! and put back when TUN stops, Prism exits, or at the next start.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};

const SERVERS: [&str; 2] = ["223.5.5.5", "119.29.29.29"];
const FILE: &str = "dns-backup.json";

/// Backup file of the override in effect.
static ACTIVE: Mutex<Option<PathBuf>> = Mutex::new(None);

fn networksetup(args: &[&str]) -> Result<String> {
    let out = Command::new("/usr/sbin/networksetup").args(args).output().context("run networksetup")?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() || text.contains("** Error") {
        bail!("networksetup {}: {}", args.join(" "), text.trim());
    }
    Ok(text)
}

/// A service's DNS servers; empty when it uses the ones from DHCP.
fn current(service: &str) -> Vec<String> {
    networksetup(&["-getdnsservers", service])
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| l.parse::<std::net::IpAddr>().is_ok())
        .map(String::from)
        .collect()
}

fn set(service: &str, servers: &[String]) -> Result<()> {
    let mut args = vec!["-setdnsservers", service];
    if servers.is_empty() {
        args.push("Empty"); // back to DHCP
    } else {
        args.extend(servers.iter().map(String::as_str));
    }
    networksetup(&args).map(|_| ())
}

/// Points every enabled service at the public servers, saving what was
/// there (an existing backup is kept: it holds the real originals).
pub fn override_dns(dir: &Path) -> Result<()> {
    let file = dir.join(FILE);
    let services = crate::sysproxy::services();
    let mut saved: BTreeMap<String, Vec<String>> = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let ours: Vec<String> = SERVERS.iter().map(|s| s.to_string()).collect();
    for s in &services {
        let now = current(s);
        if now != ours {
            saved.entry(s.clone()).or_insert(now);
        }
    }
    std::fs::write(&file, serde_json::to_vec_pretty(&saved)?)?;
    *ACTIVE.lock().unwrap() = Some(file);
    let errors: Vec<String> = services.iter().filter_map(|s| set(s, &ours).err().map(|e| format!("{e:#}"))).collect();
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

/// Puts back the settings saved in `dir`, if any. One service failing (it
/// was deleted, say) doesn't stop the others; only the failed ones stay in
/// the backup for the next try.
pub fn restore_from(dir: &Path) -> Result<()> {
    let file = dir.join(FILE);
    let Ok(bytes) = std::fs::read(&file) else { return Ok(()) };
    let saved: BTreeMap<String, Vec<String>> = serde_json::from_slice(&bytes).unwrap_or_default();
    let mut failed = BTreeMap::new();
    let mut errors = Vec::new();
    for (service, servers) in saved {
        if let Err(e) = set(&service, &servers) {
            errors.push(format!("{e:#}"));
            failed.insert(service, servers);
        }
    }
    if failed.is_empty() {
        std::fs::remove_file(&file)?;
        return Ok(());
    }
    std::fs::write(&file, serde_json::to_vec_pretty(&failed)?)?;
    bail!("{}", errors.join("; "))
}

/// Undoes the override made by this run, if any.
pub fn restore() {
    let file = ACTIVE.lock().unwrap().take();
    if let Some(dir) = file.as_deref().and_then(Path::parent) {
        if let Err(e) = restore_from(dir) {
            eprintln!("[prism] 恢复 DNS 设置失败: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    /// Only a service that doesn't exist, so no real setting is touched.
    #[test]
    fn failed_restore_keeps_only_the_failures() {
        let dir = std::env::temp_dir().join(format!("prism-macdns-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(super::FILE);
        std::fs::write(&file, r#"{"Prism No Such Service": []}"#).unwrap();
        let err = super::restore_from(&dir).unwrap_err().to_string();
        assert!(err.contains("Prism No Such Service"), "{err}");
        assert!(std::fs::read_to_string(&file).unwrap().contains("Prism No Such Service"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
