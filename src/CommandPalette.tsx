import { useEffect, useMemo, useRef, useState } from "react";
import { api, type Mode, type ProxyItem } from "./api";
import { useApp } from "./state";
import type { PageKey } from "./App";
import { t } from "./i18n";

interface Item {
  id: string;
  title: string;
  /** Shown on the right: group, current state… */
  hint?: string;
  kind: "page" | "action" | "node";
  /** Extra text to match (English names, pinyin-free aliases). */
  keywords?: string;
  run: () => void | Promise<unknown>;
}

const isMac = /Mac|iPhone|iPad/.test(navigator.platform);
export const PALETTE_KEY = isMac ? "⌘K" : "Ctrl+K";

/** Matches every space-separated word, case-insensitively; lower = better. */
function score(item: Item, q: string): number | null {
  if (!q) return 0;
  const hay = `${item.title} ${item.hint ?? ""} ${item.keywords ?? ""}`.toLowerCase();
  let total = 0;
  for (const w of q.toLowerCase().split(/\s+/).filter(Boolean)) {
    const i = hay.indexOf(w);
    if (i < 0) return null;
    total += i;
  }
  // Titles that start with the query come first.
  return item.title.toLowerCase().startsWith(q.toLowerCase()) ? total - 1000 : total;
}

/** Ctrl/⌘+K: jump to a page, switch mode / TUN / system proxy, or pick a node. */
export default function CommandPalette({ pages, go }: { pages: readonly { key: PageKey; label: string; icon: string }[]; go: (p: PageKey) => void }) {
  const { snap, run } = useApp();
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const [proxies, setProxies] = useState<Record<string, ProxyItem>>({});
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!open) return;
    setQ("");
    setSel(0);
    setTimeout(() => input.current?.focus(), 0);
    if (snap?.running) {
      api
        .core<{ proxies: Record<string, ProxyItem> }>("GET", "/proxies")
        .then((r) => setProxies(r.proxies))
        .catch(() => setProxies({}));
    }
  }, [open, snap?.running]);

  const items = useMemo(() => {
    if (!snap) return [];
    const s = snap.settings;
    const out: Item[] = pages.map((p) => ({
      id: `page:${p.key}`,
      title: t(p.label),
      hint: t("页面"),
      kind: "page",
      keywords: p.key,
      run: () => go(p.key),
    }));
    const modes: [Mode, string, string][] = [
      ["rule", "规则模式", "rule"],
      ["global", "全局模式", "global"],
      ["direct", "直连模式", "direct"],
    ];
    for (const [m, label, kw] of modes) {
      out.push({
        id: `mode:${m}`,
        title: t("切换到{mode}", { mode: t(label) }),
        hint: s.mode === m ? t("当前") : undefined,
        kind: "action",
        keywords: `mode ${kw} 模式`,
        run: () => run(() => api.setMode(m)),
      });
    }
    out.push({
      id: "tun",
      title: s.tun ? t("关闭 TUN 模式") : t("开启 TUN 模式"),
      hint: s.tun ? t("已开启") : t("已关闭"),
      kind: "action",
      keywords: "tun 虚拟网卡",
      run: () => run(() => api.setTun(!s.tun), s.tun ? t("TUN 模式已关闭") : t("TUN 模式已开启")),
    });
    out.push({
      id: "sysproxy",
      title: snap.system_proxy ? t("关闭系统代理") : t("开启系统代理"),
      hint: snap.system_proxy ? t("已开启") : t("已关闭"),
      kind: "action",
      keywords: "system proxy 系统代理",
      run: () => run(() => api.setSystemProxy(!snap.system_proxy)),
    });
    out.push({
      id: "core",
      title: snap.running ? t("停止内核") : t("启动内核"),
      kind: "action",
      keywords: "core start stop 内核",
      run: () => run(() => (snap.running ? api.stop() : api.start())),
    });
    // Nodes of the groups that can be switched by hand.
    const groups = (proxies["GLOBAL"]?.all ?? Object.keys(proxies)).map((n) => proxies[n]).filter((g): g is ProxyItem => !!g?.all && g.type === "Selector" && !g.hidden);
    if (s.mode === "global" && proxies["GLOBAL"]) groups.unshift(proxies["GLOBAL"]);
    for (const g of groups) {
      for (const n of g.all ?? []) {
        out.push({
          id: `node:${g.name}:${n}`,
          title: n,
          hint: g.now === n ? `${g.name} · ${t("当前")}` : g.name,
          kind: "node",
          run: () => run(() => api.selectProxy(g.name, n)),
        });
      }
    }
    return out;
  }, [snap, proxies, pages, go, run]);

  const shown = useMemo(() => {
    const scored = items.map((it) => [it, score(it, q.trim())] as const).filter(([, sc]) => sc !== null) as [Item, number][];
    // Without a query: pages and actions only (nodes would flood the list).
    const pool = q.trim() ? scored : scored.filter(([it]) => it.kind !== "node");
    return pool.sort((a, b) => a[1] - b[1]).slice(0, 60).map(([it]) => it);
  }, [items, q]);

  useEffect(() => setSel(0), [q]);
  useEffect(() => {
    list.current?.querySelector(".pal-item.sel")?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  if (!open) return null;
  const choose = (it?: Item) => {
    if (!it) return;
    setOpen(false);
    it.run();
  };
  const KIND: Record<Item["kind"], string> = { page: "↗", action: "⚡", node: "◆" };

  return (
    <div className="modal-bg palette-bg" onClick={() => setOpen(false)}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={input}
          className="palette-input"
          placeholder={t("搜索页面、操作或节点…")}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSel((i) => Math.min(shown.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSel((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              choose(shown[sel]);
            } else if (e.key === "Escape") {
              setOpen(false);
            }
          }}
        />
        <div className="palette-list" ref={list}>
          {shown.length === 0 && <div className="muted small" style={{ padding: 12 }}>{t("没有匹配的项目")}</div>}
          {shown.map((it, i) => (
            <div key={it.id} className={`pal-item ${i === sel ? "sel" : ""}`} onMouseMove={() => setSel(i)} onClick={() => choose(it)}>
              <span className={`pal-kind ${it.kind}`}>{KIND[it.kind]}</span>
              <span className="pal-title">{it.title}</span>
              {it.hint && <span className="pal-hint">{it.hint}</span>}
            </div>
          ))}
        </div>
        <div className="palette-foot small muted">
          <span>↑↓ {t("选择")}</span>
          <span>Enter {t("执行")}</span>
          <span>Esc {t("关闭")}</span>
          <span className="spacer" />
          <span>{snap?.running ? t("输入节点名可直接切换节点") : t("内核运行时可搜索节点")}</span>
        </div>
      </div>
    </div>
  );
}
