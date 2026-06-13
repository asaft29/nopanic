use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

const HERO: &str = r#"                               _
                              (_)
 ____   ___  ____  _____ ____  _  ____
|  _ \ / _ \|  _ \(____ |  _ \| |/ ___)
| | | | |_| | |_| / ___ | | | | ( (___
|_| |_|\___/|  __/\_____|_| |_|_|\____)
            |_|
"#;

// ── Data models (unchanged from API) ──────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct NodeMetricsUI {
    pub connections_accepted: u64,
    pub circuits_active: u64,
    pub circuits_created: u64,
    pub circuits_destroyed: u64,
    pub bytes_forwarded: u64,
    pub bytes_received: u64,
    pub streams_opened: u64,
    pub uptime_secs: u64,
    #[serde(default)]
    pub event_snapshot: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub node_type: String,
    pub address: String,
    pub bandwidth: u64,
    #[allow(dead_code)]
    pub metrics: Option<NodeMetricsUI>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Stats {
    pub total_nodes: usize,
    pub entry_count: usize,
    pub middle_count: usize,
    pub exit_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetricsSummary {
    pub registrations: u64,
    pub removals: u64,
    pub heartbeats: u64,
    pub path_requests: u64,
    pub stale_cleaned: u64,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventEntry {
    pub elapsed_secs: f64,
    pub event_type: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DashboardData {
    pub nodes: Vec<NodeInfo>,
    pub stats: Stats,
    pub metrics: MetricsSummary,
    pub ready: bool,
    pub events: Vec<EventEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section { Dashboard, About }

// ── Sorting / filtering ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum TabKind { All, Entry, Middle, Exit }

#[derive(Debug, Clone, Copy, PartialEq)]
enum SortCol { Id, Type, Addr }

#[derive(Debug, Clone, Copy, PartialEq)]
enum EventFilter { All, Register, Heartbeat, Path, Error }

impl EventFilter {
    fn matches(&self, event_type: &str) -> bool {
        match self {
            EventFilter::All => true,
            EventFilter::Register => event_type == "register",
            EventFilter::Heartbeat => event_type == "heartbeat",
            EventFilter::Path => event_type == "path",
            EventFilter::Error => event_type == "error",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            EventFilter::All => "All",
            EventFilter::Register => "Register",
            EventFilter::Heartbeat => "Heartbeat",
            EventFilter::Path => "Path",
            EventFilter::Error => "Error",
        }
    }

    fn all() -> [EventFilter; 5] {
        [EventFilter::All, EventFilter::Register, EventFilter::Heartbeat, EventFilter::Path, EventFilter::Error]
    }
}

// ── API fetch ─────────────────────────────────────────────────────

async fn fetch_dashboard() -> Option<DashboardData> {
    let resp = Request::get("/api/dashboard").send().await.ok()?;
    resp.json::<DashboardData>().await.ok()
}

// ── Formatting helpers ────────────────────────────────────────────

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB { format!("{:.1} GB", bytes as f64 / GB as f64) }
    else if bytes >= MB { format!("{:.1} MB", bytes as f64 / MB as f64) }
    else if bytes >= KB { format!("{:.1} KB", bytes as f64 / KB as f64) }
    else { format!("{bytes} B") }
}

fn format_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn format_elapsed(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("+{h}:{m:02}:{s:02}")
}

// ── Node table helpers ────────────────────────────────────────────

fn filter_nodes(nodes: &[NodeInfo], tab: TabKind) -> Vec<NodeInfo> {
    nodes.iter()
        .filter(|n| match tab {
            TabKind::All => true,
            TabKind::Entry => n.node_type == "Entry",
            TabKind::Middle => n.node_type == "Middle",
            TabKind::Exit => n.node_type == "Exit",
        })
        .cloned()
        .collect()
}

fn sort_nodes(nodes: &mut [NodeInfo], col: SortCol, asc: bool) {
    match col {
        SortCol::Id => nodes.sort_by(|a, b| if asc { a.node_id.cmp(&b.node_id) } else { b.node_id.cmp(&a.node_id) }),
        SortCol::Type => nodes.sort_by(|a, b| if asc { a.node_type.cmp(&b.node_type) } else { b.node_type.cmp(&a.node_type) }),
        SortCol::Addr => nodes.sort_by(|a, b| if asc { a.address.cmp(&b.address) } else { b.address.cmp(&a.address) }),
    }
}

fn health_class(metrics: &Option<NodeMetricsUI>) -> &'static str {
    match metrics {
        Some(m) if m.uptime_secs > 0 => "health-alive",
        Some(_) => "health-stale",
        None => "health-dead",
    }
}

// ── Components ─────────────────────────────────────────────────────

#[component]
fn NodeRow(node: NodeInfo, selected_id: ReadSignal<Option<String>>, on_select: Callback<String>) -> impl IntoView {
    let nid = node.node_id.clone();
    let short_id: String = nid.chars().take(8).collect();
    let type_class = format!("type-badge type-{}", match node.node_type.as_str() {
        "Entry" => "entry", "Middle" => "middle", "Exit" => "exit", _ => ""
    });
    let nid_for_row = nid.clone();
    let selected = move || selected_id.get().as_ref() == Some(&nid);
    let health = health_class(&node.metrics);
    let up = node.metrics.as_ref().map(|m| format_uptime(m.uptime_secs)).unwrap_or_default();

    view! {
        <tr class="node-row" class:row-selected=selected on:click=move |_| on_select.run(nid_for_row.clone())>
            <td class="mono" style="text-align:center" title=node.node_id.clone()>{short_id}"..."</td>
            <td style="text-align:center"><span class=type_class>{node.node_type.clone()}</span></td>
            <td class="mono" style="text-align:center;font-size:12px">{node.address.clone()}</td>
            <td style="text-align:center;font-size:11px;color:var(--text-dim)">{up}</td>
            <td style="text-align:center"><span class="health-dot" class:health-alive={health == "health-alive"} class:health-stale={health == "health-stale"} class:health-dead={health == "health-dead"}></span></td>
        </tr>
    }
}

#[component]
fn EventRow(entry: EventEntry) -> impl IntoView {
    let time_str = format_elapsed(entry.elapsed_secs);
    let label_class = format!("log-label ev-{}", entry.event_type);

    view! {
        <div class="log-row">
            <span class="log-time">{time_str}</span>
            <span class=label_class>{entry.label}</span>
            <span class="log-detail">{entry.detail}</span>
        </div>
    }
}

fn relay_event_color(ev: &str) -> &'static str {
    if ev.contains("\u{2190} ACCEPT")      { "rel-accept" }
    else if ev.contains("\u{2699} CREATE") { "rel-create" }
    else if ev.contains("\u{2192} EXTEND") { "rel-extend" }
    else if ev.contains("\u{2717} DESTROY"){ "rel-destroy" }
    else if ev.contains("\u{2717} ERROR")  { "rel-error" }
    else if ev.contains("\u{2192} STREAM") { "rel-stream" }
    else if ev.contains("\u{2014} END")    { "rel-end" }
    else if ev.contains("\u{2014} CLOSED") { "rel-end" }
    else if ev.contains("\u{2194} DATA")   { "rel-data" }
    else if ev.contains("\u{2192} RELAY\u{2192}") { "rel-fwd" }
    else if ev.contains("\u{2190} RELAY\u{2190}") { "rel-bwd" }
    else { "rel-default" }
}

fn split_relay_event(ev: &str) -> (String, String, String) {
    let (ts, rest) = if let Some(pos) = ev.find("] ") {
        let ts = &ev[..=pos];
        let rest = ev[pos + 2..].trim_start();
        (ts, rest)
    } else {
        return (String::new(), ev.to_string(), String::new());
    };
    if let Some(pos) = rest.find("    ") {
        let label = rest[..pos].to_string();
        let detail = rest[pos..].trim_start().to_string();
        (ts.to_string(), label, detail)
    } else {
        (ts.to_string(), rest.to_string(), String::new())
    }
}

// ── About page ─────────────────────────────────────────────────────

#[component]
fn AboutPage() -> impl IntoView {
    view! {
        <div class="about">
            <p>"Hello there,"</p>
            <p>
                <a href="https://github.com/asaft29/nopanic" target="_blank"><span class="brand-name">"nopanic"</span></a>
                " is my bachelor's thesis: a Tor-like onion routing "
                "system built entirely in Rust. I've been writing Rust for "
                "almost two years, and this project brings together everything "
                "I love about the language: safety, performance, and expressive "
                "type systems."
            </p>
            <p>
                "Under the hood, "
                <a href="https://github.com/asaft29/nopanic" target="_blank"><span class="brand-name">"nopanic"</span></a>
                " runs as three microservices: a gRPC "
                "discovery service that tracks live relay nodes, relay nodes "
                "that build encrypted three-hop circuits using ntor authenticated "
                "key exchange, and a SOCKS5 client that tunnels traffic over the "
                "network. Every message travels in fixed-size 514-byte cells with "
                "AES-128-CTR encryption and SHA-256 digest integrity. "
                "Nine relay nodes run on a DigitalOcean droplet (three entry, "
                "three middle, three exit) behind a "
                <a href="https://github.com/caddyserver/caddy" target="_blank">"Caddy"</a>
                " reverse proxy with automatic HTTPS."
            </p>
            <p>
                <a href="https://github.com/asaft29/nopanic" target="_blank"><span class="brand-name">"nopanic"</span></a>
                " also features a feature-gated transport layer supporting TLS and raw TCP modes, a "
                "live web dashboard, and interactive TUI dashboards built with "
                <a href="https://ratatui.rs" target="_blank">"ratatui"</a>
                " for every component."
            </p>
            <p>
                "I also wrote "
                <a href="https://crates.io/crates/simple_socks5" target="_blank">"simple_socks5"</a>
                " as a side project for the thesis, a lightweight SOCKS5 proxy "
                "library for Rust that handles the client-side connection and "
                "authentication for "
                <a href="https://github.com/asaft29/nopanic" target="_blank"><span class="brand-name">"nopanic"</span></a>"."
            </p>
            <div class="about-badges">
                <a href="https://docs.rs/simple_socks5/0.1.2/simple_socks5" target="_blank" class="link-badge">"docs.rs"</a>
                <a href="https://crates.io/crates/simple_socks5" target="_blank" class="link-badge">"crates.io"</a>
                <a href="https://github.com/asaft29/simple_socks5" target="_blank" class="link-badge">"GitHub"</a>
            </div>
            <div class="about-closing">
                <p>
                    "No panics, no unwraps. "
                    <a href="https://doc.rust-lang.org/clippy/" target="_blank">"Clippy"</a>
                    " would be proud."
                </p>
            </div>
        </div>
    }
}

// ── Main app ───────────────────────────────────────────────────────

#[component]
pub fn App() -> impl IntoView {
    let data: RwSignal<Option<DashboardData>> = RwSignal::new(None);
    let active_tab: RwSignal<TabKind> = RwSignal::new(TabKind::All);
    let sort_col: RwSignal<SortCol> = RwSignal::new(SortCol::Id);
    let sort_asc: RwSignal<bool> = RwSignal::new(true);
    let selected_id: RwSignal<Option<String>> = RwSignal::new(None);
    let all_events: RwSignal<Vec<EventEntry>> = RwSignal::new(Vec::new());
    let latest_elapsed: RwSignal<f64> = RwSignal::new(0.0);
    let countdown: RwSignal<u8> = RwSignal::new(20);
    let event_filter: RwSignal<EventFilter> = RwSignal::new(EventFilter::All);
    let section: RwSignal<Section> = RwSignal::new(Section::Dashboard);

    // Polling loop
    spawn_local(async move {
        loop {
            if let Some(d) = fetch_dashboard().await {
                let mut events = all_events.get();
                let threshold = latest_elapsed.get();
                let mut fresh: Vec<EventEntry> = d
                    .events
                    .iter()
                    .filter(|e| e.elapsed_secs > threshold)
                    .cloned()
                    .collect();
                fresh.sort_by(|a, b| a.elapsed_secs.partial_cmp(&b.elapsed_secs).unwrap());
                let mut new_threshold = threshold;
                for evt in fresh {
                    new_threshold = evt.elapsed_secs;
                    events.push(evt);
                }
                latest_elapsed.set(new_threshold);
                events.truncate(200);
                all_events.set(events);
                data.set(Some(d));
            }
            for i in (1..=20).rev() {
                countdown.set(i);
                gloo_timers::future::sleep(std::time::Duration::from_millis(1000)).await;
            }
        }
    });

    let on_sort = move |col: SortCol| {
        if sort_col.get() == col {
            sort_asc.update(|v| *v = !*v);
        } else {
            sort_col.set(col);
            sort_asc.set(true);
        }
    };

    view! {
        <div class="container">

            // ── Hero ───────────────────────────────
            <div class="hero">
                <div class="hero-ascii">{HERO}</div>
                <p class="brand-subtitle">
                    "A Tor-like " <strong>"onion routing"</strong> " system in Rust"
                </p>
            </div>

            // ── Nav bar ────────────────────────────
            <div class="navbar">
                <button class="nav-btn" class:nav-active=move || section.get() == Section::Dashboard
                    on:click=move |_| section.set(Section::Dashboard)>"Dashboard"</button>
                <button class="nav-btn" class:nav-active=move || section.get() == Section::About
                    on:click=move |_| section.set(Section::About)>"About"</button>
            </div>

            // ── Content ────────────────────────────
            {move || {
                if section.get() == Section::Dashboard {
                    view! {
                        // ── Stats strip ────────────────────────
            {move || data.get().map(|d| {
                let uptime = format_uptime(d.metrics.uptime_secs);
                view! {
                    <div class="stats-strip">
                        <span class="stat-group">"Up: "<strong>{uptime}</strong></span>
                        <span class="stat-group">"Nodes: "<strong>{d.stats.total_nodes}</strong></span>
                        <span class="stat-group">"E:"<strong>{d.stats.entry_count}</strong> " M:"<strong>{d.stats.middle_count}</strong> " X:"<strong>{d.stats.exit_count}</strong></span>
                    </div>
                }
            })}

            // ── Tab row ─────────────────────────────
            <div class="tab-row">
                <div class="tab-bar">
                    <button class="tab-btn" class:tab-active=move || active_tab.get() == TabKind::All
                        on:click=move |_| { active_tab.set(TabKind::All); selected_id.set(None); }>"All Nodes"</button>
                    <button class="tab-btn" class:tab-active=move || active_tab.get() == TabKind::Entry
                        on:click=move |_| { active_tab.set(TabKind::Entry); selected_id.set(None); }>"Entry"</button>
                    <button class="tab-btn" class:tab-active=move || active_tab.get() == TabKind::Middle
                        on:click=move |_| { active_tab.set(TabKind::Middle); selected_id.set(None); }>"Middle"</button>
                    <button class="tab-btn" class:tab-active=move || active_tab.get() == TabKind::Exit
                        on:click=move |_| { active_tab.set(TabKind::Exit); selected_id.set(None); }>"Exit"</button>
                </div>
            </div>

            // ── Node table ──────────────────────────
            <div class="node-panel">
                <table class="node-table">
                    <thead>
                        <tr>
                            <th style="cursor:pointer;text-align:center" on:click=move |_| on_sort(SortCol::Id)>
                                "ID" {move || if sort_col.get() == SortCol::Id { if sort_asc.get() { " ▲" } else { " ▼" } } else { "" }}
                            </th>
                            <th style="cursor:pointer;text-align:center" on:click=move |_| on_sort(SortCol::Type)>
                                "Type" {move || if sort_col.get() == SortCol::Type { if sort_asc.get() { " ▲" } else { " ▼" } } else { "" }}
                            </th>
                            <th style="cursor:pointer;text-align:center" on:click=move |_| on_sort(SortCol::Addr)>
                                "Addr" {move || if sort_col.get() == SortCol::Addr { if sort_asc.get() { " ▲" } else { " ▼" } } else { "" }}
                            </th>
                            <th style="text-align:center">"Up"</th>
                            <th style="text-align:center">"Health"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || {
                            let d = data.get();
                            match &d {
                                Some(d) => {
                                    let mut nodes = filter_nodes(&d.nodes, active_tab.get());
                                    sort_nodes(&mut nodes, sort_col.get(), sort_asc.get());
                                    if nodes.is_empty() {
                                        view! { <tr><td colspan="5" class="empty-cell">"No nodes matching filter"</td></tr> }.into_any()
                                    } else {
                                        nodes.into_iter().map(move |n| {
                                            let sel = selected_id;
                                            view! { <NodeRow node=n selected_id=sel.read_only() on_select=Callback::new(move |id: String| { if selected_id.get() == Some(id.clone()) { selected_id.set(None) } else { selected_id.set(Some(id)) }})/> }
                                        }).collect_view().into_any()
                                    }
                                }
                                None => view! { <tr><td colspan="5" class="empty-cell">"Connecting to discovery service..."</td></tr> }.into_any()
                            }
                        }}
                    </tbody>
                </table>
            </div>

            // ── Detail panel (per-node) ─────────────
            {move || {
                if let (Some(ref id), Some(ref d)) = (selected_id.get(), data.get()) {
                    if let Some(n) = d.nodes.iter().find(|n| n.node_id == *id) {
                        let m = n.metrics.as_ref();
                        let uptime = m.map(|m| format_uptime(m.uptime_secs)).unwrap_or_default();
                        let conns = m.map(|m| m.connections_accepted.to_string()).unwrap_or_default();
                        let streams = m.map(|m| m.streams_opened.to_string()).unwrap_or_default();
                        let created = m.map(|m| m.circuits_created.to_string()).unwrap_or_default();
                        let destroyed = m.map(|m| m.circuits_destroyed.to_string()).unwrap_or_default();
                        let fwd = m.map(|m| format_bytes(m.bytes_forwarded)).unwrap_or_default();
                        let recv = m.map(|m| format_bytes(m.bytes_received)).unwrap_or_default();
                        let relay_events = m.map(|m| m.event_snapshot.clone()).unwrap_or_default();
                        let short_id: String = n.node_id.chars().take(8).collect();
                        let ntype = n.node_type.clone();
                        let addr = n.address.clone();
                        return view! {
                            <div class="bottom-panel">
                                <div class="bottom-header">
                                    <span class="tab-label">"Node: " {short_id}"..."</span>
                                    <span class="stat" style="margin-left:8px">{ntype}</span>
                                    <span class="stat" style="margin-left:8px">{addr}</span>
                                    <span class="stat" style="margin-left:8px">"Up: "<strong>{uptime}</strong></span>
                                    <span class="stat" style="margin-left:8px">"Conn: "<strong>{conns}</strong></span>
                                    <span class="stat" style="margin-left:8px">"Str: "<strong>{streams}</strong></span>
                                    <span class="stat" style="margin-left:8px">"Circ: "<strong>{created}"/"{destroyed}</strong></span>
                                    <span class="stat" style="margin-left:8px">"Fwd: "<strong>{fwd}</strong></span>
                                    <span class="stat" style="margin-left:8px">"Rec: "<strong>{recv}</strong></span>
                                </div>
                                <div class="log-panel" style="max-height:200px;border:1px solid var(--border);border-radius:6px;margin-top:6px">
                                    {if relay_events.is_empty() {
                                        view! { <div class="log-empty-msg">"Waiting for relay events..."</div> }.into_any()
                                    } else {
                                        relay_events.into_iter().rev().take(200).map(|ev| {
                                            let color = relay_event_color(&ev);
                                            let (ts, label, detail) = split_relay_event(&ev);
                                            view! {
                                                <div class="log-row">
                                                    <span class="rel-time">{ts}" "</span>
                                                    <span class=color.to_string()>{label}"    "</span>
                                                    <span class="rel-detail">{detail}</span>
                                                </div>
                                            }
                                        }).collect_view().into_any()
                                    }}
                                </div>
                            </div>
                        }.into_any();
                    }
                }
                view! {}.into_any()
            }}

            // ── Activity log ────────────────────────
            <div class="log-panel-side">
                <div class="tab-label-inactive">
                    "Activity Log"
                    {move || view! { <span class="log-count">" (" {all_events.get().len()} ")"</span> }}
                </div>
                <div class="event-filter-bar">
                    {EventFilter::all().iter().map(|f| {
                        let f = *f;
                        view! {
                            <button class="event-filter-btn"
                                class:filter-active=move || event_filter.get() == f
                                on:click=move |_| event_filter.set(f)>
                                {f.label()}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>
                <div class="log-panel">
                    {move || {
                        let evts = all_events.get();
                        let filter = event_filter.get();
                        let filtered: Vec<_> = evts.iter()
                            .filter(|e| filter.matches(&e.event_type))
                            .cloned()
                            .collect();
                        if filtered.is_empty() {
                            view! { <div class="log-empty-msg">"No events matching filter"</div> }.into_any()
                        } else {
                            filtered.into_iter().rev().map(|e| view! { <EventRow entry=e/> }).collect_view().into_any()
                        }
                    }}
                </div>
            </div>

            // ── Status bar ──────────────────────────
            <div class="status-bar">
                {move || {
                    let c = countdown.get();
                    let filled = "\u{2588}".repeat(c as usize);
                    let empty = "\u{2591}".repeat(20usize.saturating_sub(c as usize));
                    view! { <span class="countdown-bar">{filled}{empty}</span> }.into_any()
                }}
            </div>
                    }.into_any()
                } else {
                    view! { <AboutPage/> }.into_any()
                }
            }}

            // ── Footer bar ─────────────────────────
            <div class="footer-bar">
                <a href="https://github.com/asaft29" target="_blank">
                    <svg class="gh-icon" viewBox="0 0 16 16" fill="currentColor">
                        <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38
                        0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13
                        -.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66
                        .07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15
                        -.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0
                        1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82
                        1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01
                        1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"/>
                    </svg>
                    "github.com/asaft29"
                </a>
            </div>

        </div>
    }
}
