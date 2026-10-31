import { useEffect, useMemo, useRef, useState } from "react";
import { geoCentroid, geoNaturalEarth1, geoPath } from "d3-geo";
import { feature } from "topojson-client";
import type { Feature, FeatureCollection, Geometry } from "geojson";
import type { GeometryCollection, Topology } from "topojson-specification";
import world from "world-atlas/countries-110m.json";
import { api, fmtBytes, fmtSpeed, type Connection } from "../api";
import { COUNTRIES, homeCountry } from "../geo-countries";
import { getLang, t } from "../i18n";

interface Row extends Connection {
  upSpeed: number;
  downSpeed: number;
}

const W = 960;
const H = 470;
const BUILTIN = ["DIRECT", "REJECT", "REJECT-DROP", "PASS", "COMPATIBLE"];

const topo = world as unknown as Topology<{ countries: GeometryCollection<{ name: string }> }>;
const shapes = (feature(topo, topo.objects.countries) as FeatureCollection<Geometry, { name: string }>).features.filter(
  (f) => f.id !== "010", // Antarctica
);
const projection = geoNaturalEarth1().fitExtent(
  [
    [6, 6],
    [W - 6, H - 6],
  ],
  { type: "FeatureCollection", features: shapes } as FeatureCollection,
);
const path = geoPath(projection);
const byId = new Map(shapes.map((f) => [String(f.id), f]));
const countryPaths = shapes.map((f) => ({ id: String(f.id), d: path(f) ?? "" }));

/** Where a country's marker goes, in map pixels. */
const pointCache = new Map<string, [number, number] | null>();
function pointOf(code: string): [number, number] | null {
  if (pointCache.has(code)) return pointCache.get(code)!;
  const c = COUNTRIES[code];
  let lonlat: [number, number] | null = c?.[2] ?? null;
  if (!lonlat && c?.[0] && byId.get(c[0])) lonlat = geoCentroid(byId.get(c[0]) as Feature);
  const p = lonlat ? (projection(lonlat) as [number, number] | null) : null;
  pointCache.set(code, p);
  return p;
}

/** Country name in the interface language (English names from the browser). */
function nameOf(code: string): string {
  if (getLang() === "en") {
    try {
      return new Intl.DisplayNames(["en"], { type: "region" }).of(code) ?? code;
    } catch {
      return code;
    }
  }
  return COUNTRIES[code]?.[1] ?? code;
}

/** A curved path from a to b, bowed upwards (more for longer arcs). */
function arc(a: [number, number], b: [number, number]) {
  const [x1, y1] = a;
  const [x2, y2] = b;
  const dx = x2 - x1;
  const dy = y2 - y1;
  const dist = Math.hypot(dx, dy);
  const lift = Math.min(160, dist * 0.35);
  const cx = (x1 + x2) / 2 - (dy / (dist || 1)) * lift * 0.3;
  const cy = (y1 + y2) / 2 - lift;
  return `M${x1.toFixed(1)},${y1.toFixed(1)} Q${cx.toFixed(1)},${cy.toFixed(1)} ${x2.toFixed(1)},${y2.toFixed(1)}`;
}

interface Spot {
  code: string;
  conns: number;
  speed: number;
  bytes: number;
}

interface Flow {
  from: string;
  to: string;
  kind: "direct" | "hop" | "exit";
  conns: number;
  speed: number;
}

/** 连接地图: where connections go, through which node's country. */
export default function ConnMap({ rows }: { rows: Row[] }) {
  const [geo, setGeo] = useState<Map<string, string | null>>(new Map());
  const [servers, setServers] = useState<Record<string, string>>({});
  const [error, setError] = useState("");
  const [hover, setHover] = useState<string | null>(null);
  const pending = useRef(false);
  const home = useMemo(homeCountry, []);

  useEffect(() => {
    api.nodeServers().then(setServers, () => setServers({}));
  }, []);

  const destOf = (c: Row) => c.metadata.destinationIP || c.metadata.host;
  const nodeOf = (c: Row) => (c.chains[0] && !BUILTIN.includes(c.chains[0].toUpperCase()) ? c.chains[0] : null);

  // Look up countries for endpoints we haven't seen.
  useEffect(() => {
    if (pending.current) return;
    const want = new Set<string>();
    for (const c of rows) {
      const d = destOf(c);
      if (d && !geo.has(d)) want.add(d);
      const n = nodeOf(c);
      const s = n && servers[n];
      if (s && !geo.has(s)) want.add(s);
    }
    if (want.size === 0) return;
    pending.current = true;
    api
      .geoLocate([...want].slice(0, 300))
      .then((r) => {
        setError("");
        setGeo((g) => {
          const next = new Map(g);
          for (const [k, v] of Object.entries(r)) next.set(k, v);
          return next;
        });
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        pending.current = false;
      });
  }, [rows, servers, geo]);

  const { spots, nodes, flows, unknown } = useMemo(() => {
    const spots = new Map<string, Spot>();
    const nodes = new Map<string, Spot & { names: Set<string> }>();
    const flows = new Map<string, Flow>();
    let unknown = 0;
    const addFlow = (from: string, to: string, kind: Flow["kind"], speed: number) => {
      if (from === to) return;
      const key = `${kind}|${from}|${to}`;
      const f = flows.get(key) ?? { from, to, kind, conns: 0, speed: 0 };
      f.conns += 1;
      f.speed += speed;
      flows.set(key, f);
    };
    for (const c of rows) {
      const dc = geo.get(destOf(c));
      const speed = c.upSpeed + c.downSpeed;
      const node = nodeOf(c);
      const nc = node && servers[node] ? geo.get(servers[node]) : null;
      if (!dc) {
        unknown += 1;
      } else {
        const s = spots.get(dc) ?? { code: dc, conns: 0, speed: 0, bytes: 0 };
        s.conns += 1;
        s.speed += speed;
        s.bytes += c.upload + c.download;
        spots.set(dc, s);
      }
      if (node && nc) {
        const n = nodes.get(nc) ?? { code: nc, conns: 0, speed: 0, bytes: 0, names: new Set<string>() };
        n.conns += 1;
        n.speed += speed;
        n.bytes += c.upload + c.download;
        n.names.add(node);
        nodes.set(nc, n);
        addFlow(home, nc, "hop", speed);
        if (dc) addFlow(nc, dc, "exit", speed);
      } else if (dc && !node) {
        addFlow(home, dc, "direct", speed);
      }
    }
    return { spots, nodes, flows: [...flows.values()], unknown };
  }, [rows, geo, servers, home]);

  const active = new Set([...spots.keys(), ...nodes.keys(), home].map((c) => COUNTRIES[c]?.[0]).filter(Boolean));
  const maxSpeed = Math.max(1, ...flows.map((f) => f.speed));
  const width = (f: Flow) => 1 + 3 * Math.sqrt(f.speed / maxSpeed) + Math.min(1.5, f.conns / 20);
  const top = [...spots.values()].sort((a, b) => b.speed - a.speed || b.conns - a.conns).slice(0, 8);
  const topNodes = [...nodes.values()].sort((a, b) => b.speed - a.speed || b.conns - a.conns).slice(0, 6);
  const homePt = pointOf(home);

  return (
    <div className="conn-map">
      <div className="conn-map-canvas card">
        <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="xMidYMid meet">
          <g className="map-land">
            {countryPaths.map((c) => (
              <path key={c.id} d={c.d} className={active.has(c.id) ? "on" : ""} />
            ))}
          </g>
          <g className="map-flows">
            {flows.map((f, i) => {
              const a = pointOf(f.from);
              const b = pointOf(f.to);
              if (!a || !b) return null;
              const dim = hover && hover !== f.to && hover !== f.from;
              return (
                <path
                  key={i}
                  d={arc(a, b)}
                  className={`flow ${f.kind} live ${dim ? "dim" : ""}`}
                  style={{ strokeWidth: width(f), animationDuration: `${Math.max(0.6, 3 - (2.4 * f.speed) / maxSpeed)}s` }}
                />
              );
            })}
          </g>
          <g className="map-spots">
            {[...nodes.values()].map((n) => {
              const p = pointOf(n.code);
              if (!p) return null;
              const r = 4 + Math.min(6, Math.sqrt(n.conns));
              return (
                <g key={`n${n.code}`} onMouseEnter={() => setHover(n.code)} onMouseLeave={() => setHover(null)}>
                  <rect x={p[0] - r} y={p[1] - r} width={r * 2} height={r * 2} transform={`rotate(45 ${p[0]} ${p[1]})`} className="node-mark" />
                  <title>{`${nameOf(n.code)} · ${[...n.names].join(", ")}\n${t("{n} 个连接", { n: n.conns })} · ${fmtSpeed(n.speed)}`}</title>
                </g>
              );
            })}
            {[...spots.values()].map((s) => {
              const p = pointOf(s.code);
              if (!p) return null;
              const r = 2.5 + Math.min(9, Math.sqrt(s.conns) * 1.4);
              return (
                <g key={`d${s.code}`} onMouseEnter={() => setHover(s.code)} onMouseLeave={() => setHover(null)}>
                  {s.speed > 0 && <circle cx={p[0]} cy={p[1]} r={r} className="dest-pulse" />}
                  <circle cx={p[0]} cy={p[1]} r={r} className="dest-mark" />
                  <title>{`${nameOf(s.code)}\n${t("{n} 个连接", { n: s.conns })} · ${fmtSpeed(s.speed)} · ${fmtBytes(s.bytes)}`}</title>
                </g>
              );
            })}
            {homePt && (
              <g>
                <circle cx={homePt[0]} cy={homePt[1]} r={6} className="home-mark" />
                <circle cx={homePt[0]} cy={homePt[1]} r={2.5} className="home-dot" />
                <title>{t("本机（按系统时区推断：{c}）", { c: nameOf(home) })}</title>
              </g>
            )}
            {top.slice(0, 5).map((s) => {
              const p = pointOf(s.code);
              return p ? (
                <text key={`l${s.code}`} x={p[0] + 8} y={p[1] - 6} className="map-label">
                  {nameOf(s.code)}
                </text>
              ) : null;
            })}
          </g>
        </svg>
        <div className="map-legend small">
          <span>
            <i className="lg home" /> {t("本机")}
          </span>
          <span>
            <i className="lg node" /> {t("节点")}
          </span>
          <span>
            <i className="lg dest" /> {t("目标")}
          </span>
          <span>
            <i className="lg line direct" /> {t("直连")}
          </span>
          <span>
            <i className="lg line hop" /> {t("到节点")}
          </span>
          <span>
            <i className="lg line exit" /> {t("节点到目标")}
          </span>
        </div>
      </div>
      <div className="conn-map-side">
        {error && <div className="notice small">{error}</div>}
        <div className="card">
          <div className="side-title">{t("目标地区")}</div>
          {top.length === 0 && <div className="muted small">{t("暂无")}</div>}
          {top.map((s) => (
            <div key={s.code} className={`side-row ${hover === s.code ? "hl" : ""}`} onMouseEnter={() => setHover(s.code)} onMouseLeave={() => setHover(null)}>
              <span className="tag mono">{s.code}</span>
              <span className="side-name">{nameOf(s.code)}</span>
              <span className="muted small">{s.conns}</span>
              <span className="side-speed small">{fmtSpeed(s.speed)}</span>
            </div>
          ))}
          {unknown > 0 && <div className="muted small">{t("{n} 个连接的位置未知（内网或未解析）", { n: unknown })}</div>}
        </div>
        <div className="card">
          <div className="side-title">{t("出口节点")}</div>
          {topNodes.length === 0 && <div className="muted small">{t("暂无经代理的连接")}</div>}
          {topNodes.map((n) => (
            <div key={n.code} className={`side-row ${hover === n.code ? "hl" : ""}`} onMouseEnter={() => setHover(n.code)} onMouseLeave={() => setHover(null)}>
              <span className="tag mono">{n.code}</span>
              <span className="side-name" title={[...n.names].join(", ")}>
                {[...n.names].join(", ")}
              </span>
              <span className="muted small">{n.conns}</span>
              <span className="side-speed small">{fmtSpeed(n.speed)}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
