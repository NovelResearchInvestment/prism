import { useState } from "react";
import { api, errText, type RuleTestResult } from "../api";
import { useApp } from "../state";
import { t } from "../i18n";

type ProxyNow = Record<string, { now?: string; type: string }>;

/** Follows group selections from `name` down to a node: [group, ..., node]. */
function chainOf(name: string, proxies: ProxyNow | null): string[] {
  const out = [name];
  let cur = name;
  while (proxies?.[cur]?.now && !out.includes(proxies[cur].now!) && out.length < 10) {
    cur = proxies[cur].now!;
    out.push(cur);
  }
  return out;
}

/** 规则测试: type a domain / IP / URL, see which rule it hits and the exit it ends at. */
export default function RuleTest() {
  const { snap } = useApp();
  const [target, setTarget] = useState("");
  const [network, setNetwork] = useState<"tcp" | "udp">("tcp");
  const [process, setProcess] = useState("");
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<{ r: RuleTestResult; chain: string[] } | null>(null);
  const [error, setError] = useState("");
  const mode = snap?.settings.mode ?? "rule";

  const test = async () => {
    if (!target.trim()) return;
    setBusy(true);
    setError("");
    try {
      const r = await api.testRule(target, network, process);
      const proxies = snap?.running ? await api.core<{ proxies: ProxyNow }>("GET", "/proxies").then((p) => p.proxies).catch(() => null) : null;
      setRes({ r, chain: chainOf(r.hit.target, proxies) });
    } catch (e) {
      setRes(null);
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card rule-test">
      <div className="row">
        <b>{t("规则测试")}</b>
        <span className="muted small">{t("输入域名、IP 或网址，查看会命中哪条规则、从哪个出口出去")}</span>
      </div>
      <div className="row">
        <input
          className="input mono"
          style={{ flex: 2 }}
          placeholder="www.google.com / 8.8.8.8 / https://chatgpt.com"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && test()}
        />
        <input
          className="input"
          style={{ flex: 1 }}
          placeholder={t("程序（可选），如 chrome.exe")}
          value={process}
          onChange={(e) => setProcess(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && test()}
        />
        <div className="seg">
          {(["tcp", "udp"] as const).map((n) => (
            <button key={n} className={network === n ? "on" : ""} onClick={() => setNetwork(n)}>
              {n.toUpperCase()}
            </button>
          ))}
        </div>
        <button className="btn primary" disabled={busy || !target.trim()} onClick={test}>
          {busy ? t("测试中…") : t("测试")}
        </button>
      </div>
      {error && <div className="small" style={{ color: "var(--bad)" }}>{error}</div>}
      {res && (
        <div className="rule-test-result">
          <div>
            <span className="muted">{t("目标")}</span>
            <span className="mono">
              {res.r.host}:{res.r.port}
              {res.r.ip && res.r.ip !== res.r.host && <span className="muted"> → {res.r.ip}</span>}
            </span>
          </div>
          <div>
            <span className="muted">{t("命中")}</span>
            {res.r.hit.index != null ? (
              <span className="mono">
                <span className="tag">#{res.r.hit.index + 1} / {res.r.total}</span> {res.r.hit.line}
              </span>
            ) : (
              <span>{t("没有规则命中，直连")}</span>
            )}
          </div>
          <div>
            <span className="muted">{t("出口")}</span>
            <span className="chain">
              {res.chain.map((c, i) => (
                <span key={i}>
                  {i > 0 && <span className="muted"> → </span>}
                  <b className={i === res.chain.length - 1 ? "accent-text" : ""}>{c}</b>
                </span>
              ))}
            </span>
          </div>
          {mode !== "rule" && (
            <div className="small" style={{ color: "var(--mid)" }}>
              {mode === "global" ? t("当前是全局模式：所有连接都走 GLOBAL，规则不生效") : t("当前是直连模式：所有连接都直连，规则不生效")}
            </div>
          )}
          {res.r.warnings.length > 0 && (
            <details className="small muted">
              <summary>{t("{n} 条提示", { n: res.r.warnings.length })}</summary>
              {res.r.warnings.map((w, i) => (
                <div key={i}>{w}</div>
              ))}
            </details>
          )}
        </div>
      )}
    </div>
  );
}
