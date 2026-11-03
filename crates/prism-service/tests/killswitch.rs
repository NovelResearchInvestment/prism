//! Opt-in, Windows only: changes the machine's firewall state while it runs, and
//! needs an administrator. Run it where a few seconds without network are fine.
//!   PRISM_KILLSWITCH_TEST=1 cargo test -p prism-service --test killswitch
//! With PRISM_KILLSWITCH_CORES set (`kind=exe;...`) it also runs each core with
//! TUN, and checks that a crash blocks the traffic instead of sending it direct.
#![cfg(windows)]

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use prism_service::killswitch::KillSwitch;

/// A public address that answers on 443.
const PUBLIC: &str = "1.1.1.1:443";

fn can_connect(addr: &str) -> bool {
    let addr: SocketAddr = addr.parse().unwrap();
    TcpStream::connect_timeout(&addr, Duration::from_secs(4)).is_ok()
}

/// A program outside the kill switch's list, connecting on its own.
fn outside_can_connect() -> bool {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "$c=New-Object Net.Sockets.TcpClient; if($c.ConnectAsync('1.1.1.1',443).Wait(4000)){exit 0}else{exit 1}",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn enabled() -> bool {
    if std::env::var_os("PRISM_KILLSWITCH_TEST").is_none() {
        eprintln!("PRISM_KILLSWITCH_TEST not set, skipping");
        return false;
    }
    true
}

#[test]
fn blocks_and_lifts() {
    if !enabled() {
        return;
    }
    assert!(can_connect(PUBLIC), "needs network to start with");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let me = std::env::current_exe().unwrap();

    let ks = KillSwitch::enable(&[]).expect("needs an administrator");
    assert!(!can_connect(PUBLIC), "public address must be blocked");
    assert!(TcpStream::connect(listener.local_addr().unwrap()).is_ok(), "loopback must stay open");
    drop(ks);
    assert!(can_connect(PUBLIC), "dropping must lift the block");

    let ks = KillSwitch::enable(&[me]).unwrap();
    assert!(can_connect(PUBLIC), "a listed program may connect");
    assert!(!outside_can_connect(), "other programs may not");
    drop(ks);
    assert!(outside_can_connect(), "dropping must lift the block");
}

/// A crash of the holder must lift the block too: that's what keeps a broken
/// service from cutting the machine off.
#[test]
fn holder_crash_lifts_the_block() {
    if !enabled() {
        return;
    }
    if std::env::var_os("PRISM_KILLSWITCH_HOLDER").is_some() {
        let _ks = KillSwitch::enable(&[]).unwrap();
        println!("held");
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    let mut holder = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "holder_crash_lifts_the_block", "--nocapture"])
        .env("PRISM_KILLSWITCH_HOLDER", "1")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let blocked = !can_connect(PUBLIC);
    holder.kill().unwrap();
    holder.wait().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(blocked, "holder must block");
    assert!(can_connect(PUBLIC), "a killed holder must not leave the block behind");
}

fn tun_config(kind: &str, dir: &std::path::Path) -> PathBuf {
    if kind == "singbox" {
        let p = dir.join("c.json");
        std::fs::write(
            &p,
            r#"{"log":{"level":"info"},"inbounds":[{"type":"tun","interface_name":"Prism","address":["172.19.0.1/30","fdfe:dcba:9876::1/126"],"auto_route":true,"stack":"mixed"}],"outbounds":[{"type":"direct","tag":"DIRECT"}],"route":{"final":"DIRECT","auto_detect_interface":true}}"#,
        )
        .unwrap();
        return p;
    }
    let p = dir.join("c.yaml");
    std::fs::write(
        &p,
        "log-level: info\nmixed-port: 17910\nfind-process-mode: off\ntun:\n  enable: true\n  stack: mixed\n  device: Prism\n  auto-route: true\n  auto-detect-interface: true\n  mtu: 9000\n  inet6-address: [fdfe:dcba:9876::1/126]\nproxies: []\nrules:\n  - MATCH,DIRECT\n",
    )
    .unwrap();
    p
}

fn spawn_core(kind: &str, exe: &str, cfg: &std::path::Path, dir: &std::path::Path) -> Child {
    let mut c = Command::new(exe);
    if kind == "singbox" {
        c.arg("run").arg("-c").arg(cfg).arg("-D").arg(dir);
    } else if kind == "prism" {
        c.arg("-f").arg(cfg).arg("-d").arg(dir).arg("--tun");
    } else {
        c.arg("-f").arg(cfg).arg("-d").arg(dir);
    }
    c.current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
}

/// The whole promise, one core at a time: traffic flows through the TUN with
/// the block on, and stops, rather than going direct, when the core dies.
#[test]
fn core_crash_blocks_tun_traffic() {
    if !enabled() {
        return;
    }
    let Ok(cores) = std::env::var("PRISM_KILLSWITCH_CORES") else {
        eprintln!("PRISM_KILLSWITCH_CORES not set, skipping");
        return;
    };
    for entry in cores.split(';').filter(|e| !e.is_empty()) {
        let (kind, exe) = entry.split_once('=').unwrap();
        let dir = std::env::temp_dir().join(format!("prism-ks-{kind}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        if let Ok(dll) = std::env::var("PRISM_KILLSWITCH_WINTUN") {
            std::fs::copy(dll, dir.join("wintun.dll")).unwrap();
        }
        let cfg = tun_config(kind, &dir);
        let ks = KillSwitch::enable(&[PathBuf::from(exe)]).unwrap();
        let mut core = spawn_core(kind, exe, &cfg, &dir);
        std::thread::sleep(Duration::from_secs(8));
        let through_tun = outside_can_connect();
        core.kill().unwrap();
        core.wait().unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let after_crash = outside_can_connect();
        drop(ks);
        std::thread::sleep(Duration::from_secs(1));
        let lifted = outside_can_connect();
        assert!(through_tun, "{kind}: traffic must flow through the TUN with the block on");
        assert!(!after_crash, "{kind}: a crashed core must leave traffic blocked, not direct");
        assert!(lifted, "{kind}: lifting must restore the network");
    }
}
