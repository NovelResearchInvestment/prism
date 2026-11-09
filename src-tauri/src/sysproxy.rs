//! System proxy: WinINet registry settings on Windows, `networksetup` on
//! macOS.

#[cfg(windows)]
pub fn set(enable: bool, server: &str, bypass: &str) -> anyhow::Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        KEY_READ | KEY_WRITE,
    )?;
    key.set_value("ProxyEnable", &(enable as u32))?;
    if enable {
        key.set_value("ProxyServer", &server.to_string())?;
        key.set_value("ProxyOverride", &bypass.to_string())?;
    }
    unsafe {
        use windows_sys::Win32::Networking::WinInet::{
            InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
        };
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null(), 0);
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0);
    }
    Ok(())
}

/// Returns (enabled, server) as currently configured in Windows.
#[cfg(windows)]
pub fn get() -> (bool, String) {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return (false, String::new());
    };
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    let server: String = key.get_value("ProxyServer").unwrap_or_default();
    (enabled == 1, server)
}

/// macOS: HTTP, HTTPS and SOCKS proxies on every enabled network service
/// (Wi-Fi, Ethernet, ...), through `networksetup`.
#[cfg(target_os = "macos")]
mod mac {
    use anyhow::{bail, Context, Result};

    fn networksetup(args: &[&str]) -> Result<String> {
        let out = std::process::Command::new("/usr/sbin/networksetup").args(args).output().context("run networksetup")?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        // networksetup reports some failures on stdout with status 0.
        if !out.status.success() || text.contains("** Error") {
            bail!("networksetup {}: {}{}", args.join(" "), text.trim(), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(text)
    }

    /// Enabled network services (disabled ones are listed with a `*`).
    pub fn services() -> Vec<String> {
        networksetup(&["-listallnetworkservices"])
            .unwrap_or_default()
            .lines()
            .skip(1) // "An asterisk (*) denotes that a network service is disabled."
            .filter(|l| !l.is_empty() && !l.starts_with('*'))
            .map(String::from)
            .collect()
    }

    /// The Windows-style bypass list (`localhost;127.*;<local>`) as macOS
    /// takes it.
    pub fn bypass_list(bypass: &str) -> Vec<String> {
        let mut v: Vec<String> = bypass
            .split([';', ',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| if s == "<local>" { "*.local".to_string() } else { s.to_string() })
            .collect();
        for must in ["localhost", "127.0.0.1"] {
            if !v.iter().any(|x| x == must) {
                v.push(must.into());
            }
        }
        v
    }

    /// Turning off needs no address (Prism passes "" on exit). One service
    /// failing doesn't stop the others: on exit especially, a service left
    /// pointing at the stopped core would cut it off.
    pub fn set(enable: bool, server: &str, bypass: &str) -> Result<()> {
        set_on(&services(), enable, server, bypass)
    }

    pub(super) fn set_on(svcs: &[String], enable: bool, server: &str, bypass: &str) -> Result<()> {
        if svcs.is_empty() {
            bail!("没有可用的网络服务");
        }
        let addr = if enable { Some(server.rsplit_once(':').context("bad proxy address")?) } else { None };
        let mut errors = Vec::new();
        for s in svcs {
            if let Err(e) = set_service(s, addr, bypass) {
                errors.push(format!("{e:#}"));
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        Ok(())
    }

    /// Sets (`addr` given) or turns off one service's proxies. Setting a
    /// proxy's address also turns it on, so that takes 4 networksetup calls
    /// per service instead of 7. Turning off tries all three states even
    /// when one fails.
    fn set_service(s: &str, addr: Option<(&str, &str)>, bypass: &str) -> Result<()> {
        if let Some((host, port)) = addr {
            networksetup(&["-setwebproxy", s, host, port])?;
            networksetup(&["-setsecurewebproxy", s, host, port])?;
            networksetup(&["-setsocksfirewallproxy", s, host, port])?;
            let mut args = vec!["-setproxybypassdomains".to_string(), s.to_string()];
            args.extend(bypass_list(bypass));
            networksetup(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
            return Ok(());
        }
        let results = ["-setwebproxystate", "-setsecurewebproxystate", "-setsocksfirewallproxystate"].map(|flag| networksetup(&[flag, s, "off"]));
        results.into_iter().collect::<Result<Vec<_>>>().map(|_| ())
    }

    /// (http, https, socks) enabled on `service`.
    #[cfg(test)]
    pub(super) fn states(service: &str) -> [bool; 3] {
        ["-getwebproxy", "-getsecurewebproxy", "-getsocksfirewallproxy"]
            .map(|flag| networksetup(&[flag, service]).unwrap_or_default().lines().any(|l| l.trim() == "Enabled: Yes"))
    }

    /// (enabled, "host:port") of the first service's HTTP proxy.
    pub fn get() -> (bool, String) {
        let Some(s) = services().into_iter().next() else { return (false, String::new()) };
        let text = networksetup(&["-getwebproxy", &s]).unwrap_or_default();
        let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
        (field("Enabled:") == "Yes", format!("{}:{}", field("Server:"), field("Port:")))
    }
}

#[cfg(target_os = "macos")]
pub use mac::{get, services, set};

#[cfg(not(any(windows, target_os = "macos")))]
pub fn set(_: bool, _: &str, _: &str) -> anyhow::Result<()> {
    anyhow::bail!("system proxy is not implemented on this platform")
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn get() -> (bool, String) {
    (false, String::new())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn bypass_list_for_macos() {
        assert_eq!(super::mac::bypass_list("localhost;127.*;10.*;<local>"), ["localhost", "127.*", "10.*", "*.local", "127.0.0.1"]);
    }

    /// On the real network services, with one that fails in between: turns
    /// the system proxy on and off again. Changes this Mac's settings for a
    /// moment and leaves every proxy off, so only with PRISM_SYSPROXY_LIVE=1
    /// and the address / bypass list the services already have, e.g.
    /// `PRISM_SYSPROXY_LIVE=1 PRISM_SYSPROXY_ADDR=127.0.0.1:7890 cargo test -p prism sysproxy -- --ignored`.
    #[test]
    #[ignore]
    fn live_round_trip_on_this_mac() {
        if std::env::var_os("PRISM_SYSPROXY_LIVE").is_none() {
            return;
        }
        let addr = std::env::var("PRISM_SYSPROXY_ADDR").unwrap_or_else(|_| "127.0.0.1:7890".into());
        let bypass = std::env::var("PRISM_SYSPROXY_BYPASS").unwrap_or_default();
        let real = super::mac::services();
        assert!(!real.is_empty());
        let mut svcs = real.clone();
        svcs.insert(svcs.len() / 2, "Prism No Such Service".into());
        let t = std::time::Instant::now();
        let on = super::mac::set_on(&svcs, true, &addr, &bypass).unwrap_err().to_string();
        let on_time = t.elapsed();
        let on_states: Vec<_> = real.iter().map(|s| (s.clone(), super::mac::states(s))).collect();
        let t = std::time::Instant::now();
        let off = super::mac::set_on(&svcs, false, &addr, &bypass).unwrap_err().to_string();
        let off_time = t.elapsed();
        let off_states: Vec<_> = real.iter().map(|s| (s.clone(), super::mac::states(s))).collect();
        assert!(on.contains("Prism No Such Service") && !real.iter().any(|s| on.contains(&format!(" {s} "))), "{on}");
        assert!(off.contains("Prism No Such Service"), "{off}");
        assert!(on_states.iter().all(|(_, st)| *st == [true; 3]), "{on_states:?}");
        assert!(off_states.iter().all(|(_, st)| *st == [false; 3]), "{off_states:?}");
        println!("{} services on ({on_time:?}) and off again ({off_time:?}); only the missing one failed", real.len());
    }

    /// Only services that don't exist, so no real setting is touched.
    #[test]
    fn one_failing_service_doesnt_stop_the_rest() {
        let svcs = ["Prism No Such A".to_string(), "Prism No Such B".to_string()];
        for enable in [false, true] {
            let err = super::mac::set_on(&svcs, enable, "127.0.0.1:7890", "").unwrap_err().to_string();
            assert!(err.contains("Prism No Such A") && err.contains("Prism No Such B"), "{err}");
        }
    }
}
