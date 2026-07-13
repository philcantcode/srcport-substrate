//! Render the reconstructed [`Graph`](crate::reconstruct::Graph) as a single
//! self-contained HTML page: a hand-laid-out SVG dataflow diagram plus the
//! tamper-evident ledger it was rebuilt from. No external assets, no JS — it
//! opens offline and is safe to commit.

use std::collections::BTreeMap;

use srcport_substrate::LedgerEntry;

use crate::reconstruct::{Edge, Graph, Node};

// layout constants (SVG user units)
const PAD: f64 = 48.0;
const NODE_W: f64 = 230.0;
const NODE_H: f64 = 88.0;
const COL_GAP: f64 = 130.0;
const ROW_GAP: f64 = 44.0;
const EXT_W: f64 = 196.0;
const EXT_H: f64 = 54.0;
const PILL_W: f64 = 150.0;
const PILL_H: f64 = 46.0;

fn col_x(col: usize) -> f64 {
    PAD + col as f64 * (NODE_W + COL_GAP)
}

fn short(id: &str) -> String {
    let body = id.strip_prefix("sha256:").unwrap_or(id);
    format!("{}…", &body[..body.len().min(10)])
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// A placed box: top-left corner plus the per-artifact anchor y-coordinates on
/// its left (inputs) and right (outputs) edges.
struct Placed {
    x: f64,
    y: f64,
    left: BTreeMap<String, f64>,
    right: BTreeMap<String, f64>,
}

impl Placed {
    fn cy(&self, h: f64) -> f64 {
        self.y + h / 2.0
    }
}

fn place_node(node: &Node, x: f64, y: f64) -> Placed {
    let mut left = BTreeMap::new();
    let n = node.inputs.len().max(1);
    for (i, s) in node.inputs.iter().enumerate() {
        left.insert(s.artifact.clone(), y + NODE_H * (i as f64 + 1.0) / (n as f64 + 1.0));
    }
    let mut right = BTreeMap::new();
    let m = node.outputs.len().max(1);
    for (i, s) in node.outputs.iter().enumerate() {
        right.insert(s.artifact.clone(), y + NODE_H * (i as f64 + 1.0) / (m as f64 + 1.0));
    }
    Placed { x, y, left, right }
}

/// Render the whole page.
pub fn render(graph: &Graph, chain: &[LedgerEntry], verified: bool) -> String {
    // ── column model: col 0 = external inputs, cols 1.. = nodes by layer ─────
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); graph.max_layer + 1];
    for (i, n) in graph.nodes.iter().enumerate() {
        columns[n.layer].push(i);
    }
    let rows = columns.iter().map(|c| c.len()).max().unwrap_or(1);
    let externals_rows = graph.externals.len();
    let canvas_rows = rows.max(externals_rows).max(1);
    let canvas_h = PAD * 2.0 + canvas_rows as f64 * NODE_H + (canvas_rows.saturating_sub(1)) as f64 * ROW_GAP;
    // one extra column of width for the terminal answer pill
    let canvas_w = col_x(graph.max_layer) + NODE_W + COL_GAP + PILL_W + PAD;

    fn column_top(count: usize, item_h: f64, canvas_h: f64) -> f64 {
        let total = count as f64 * item_h + (count.saturating_sub(1)) as f64 * ROW_GAP;
        (canvas_h - total) / 2.0
    }

    // Place external pills (column 0).
    let mut ext_pos: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let mut svg = String::new();
    {
        let top = column_top(graph.externals.len(), EXT_H, canvas_h);
        for (i, (contract, id)) in graph.externals.iter().enumerate() {
            let x = col_x(0) + (NODE_W - EXT_W) / 2.0;
            let y = top + i as f64 * (EXT_H + ROW_GAP);
            ext_pos.insert(id.clone(), (x + EXT_W, y + EXT_H / 2.0));
            svg.push_str(&external_pill(x, y, contract, id));
        }
    }

    // Place nodes column by column.
    let mut placed: BTreeMap<String, Placed> = BTreeMap::new();
    for (layer, idxs) in columns.iter().enumerate() {
        if layer == 0 {
            continue;
        }
        let top = column_top(idxs.len(), NODE_H, canvas_h);
        for (row, &ni) in idxs.iter().enumerate() {
            let node = &graph.nodes[ni];
            let x = col_x(layer);
            let y = top + row as f64 * (NODE_H + ROW_GAP);
            placed.insert(node.key.clone(), place_node(node, x, y));
        }
    }

    // ── edges (drawn first, under the boxes) ─────────────────────────────────
    let mut edges_svg = String::new();
    for e in &graph.edges {
        let (x1, y1) = edge_source(graph, e, &placed, &ext_pos);
        let (x2, y2) = edge_target(e, &placed);
        edges_svg.push_str(&flow_edge(x1, y1, x2, y2, &e.contract, &e.artifact));
    }

    // ── terminal answer pill ────────────────────────────────────────────────
    if let Some(answer) = &graph.answer {
        if let Some(term) = graph.nodes.iter().find(|n| n.outputs.iter().any(|o| &o.artifact == answer)) {
            if let Some(p) = placed.get(&term.key) {
                let ay = *p.right.get(answer).unwrap_or(&p.cy(NODE_H));
                let ax = col_x(term.layer) + NODE_W;
                let px = ax + COL_GAP;
                let py = ay - PILL_H / 2.0;
                edges_svg.push_str(&flow_edge(ax, ay, px, ay, "demo.v1.Answer", answer));
                svg.push_str(&answer_pill(px, py, answer));
            }
        }
    }

    // node boxes on top of edges
    let mut nodes_svg = String::new();
    for node in &graph.nodes {
        if let Some(p) = placed.get(&node.key) {
            nodes_svg.push_str(&node_box(node, p.x, p.y));
        }
    }

    let diagram = format!(
        r#"<svg viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="reconstructed dataflow">
  <defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
      <path d="M0 0 L10 5 L0 10 z" class="arrowhead"/>
    </marker>
  </defs>
{edges}{nodes}{extras}
</svg>"#,
        w = canvas_w,
        h = canvas_h,
        edges = edges_svg,
        nodes = nodes_svg,
        extras = svg,
    );

    page(graph, chain, verified, &diagram)
}

fn edge_source(
    graph: &Graph,
    e: &Edge,
    placed: &BTreeMap<String, Placed>,
    ext_pos: &BTreeMap<String, (f64, f64)>,
) -> (f64, f64) {
    match &e.from {
        Some(from) => {
            let p = &placed[from];
            let node = graph.nodes.iter().find(|n| &n.key == from).unwrap();
            let x = col_x(node.layer) + NODE_W;
            let y = *p.right.get(&e.artifact).unwrap_or(&p.cy(NODE_H));
            (x, y)
        }
        None => ext_pos.get(&e.artifact).copied().unwrap_or((0.0, 0.0)),
    }
}

fn edge_target(e: &Edge, placed: &BTreeMap<String, Placed>) -> (f64, f64) {
    let p = &placed[&e.to];
    let y = *p.left.get(&e.artifact).unwrap_or(&p.cy(NODE_H));
    (p.x, y)
}

fn flow_edge(x1: f64, y1: f64, x2: f64, y2: f64, contract: &str, artifact: &str) -> String {
    let dx = ((x2 - x1) * 0.5).max(40.0);
    let mx = (x1 + x2) / 2.0;
    let my = (y1 + y2) / 2.0;
    let label = format!("{}  {}", contract.rsplit('.').next().unwrap_or(contract), short(artifact));
    format!(
        r#"  <path class="edge" d="M{x1:.1} {y1:.1} C{c1x:.1} {y1:.1} {c2x:.1} {y2:.1} {x2:.1} {y2:.1}" marker-end="url(#arrow)"/>
  <text class="edge-label" x="{mx:.1}" y="{my:.1}" text-anchor="middle">{label}</text>
"#,
        c1x = x1 + dx,
        c2x = x2 - dx,
        my = my - 6.0,
        label = esc(&label),
    )
}

fn node_box(node: &Node, x: f64, y: f64) -> String {
    let ports = node
        .inputs
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let py = y + NODE_H * (i as f64 + 1.0) / (node.inputs.len().max(1) as f64 + 1.0);
            format!(r#"<circle class="port" cx="{x:.1}" cy="{py:.1}" r="4"/>"#)
        })
        .chain(node.outputs.iter().enumerate().map(|(i, _)| {
            let py = y + NODE_H * (i as f64 + 1.0) / (node.outputs.len().max(1) as f64 + 1.0);
            format!(r#"<circle class="port" cx="{:.1}" cy="{py:.1}" r="4"/>"#, x + NODE_W)
        }))
        .collect::<String>();
    format!(
        r#"  <g class="node">
    <rect x="{x:.1}" y="{y:.1}" width="{w}" height="{h}" rx="12"/>
    <text class="node-title" x="{tx:.1}" y="{t1:.1}">{key}</text>
    <text class="node-sub"   x="{tx:.1}" y="{t2:.1}">{module}@{version}</text>
    <text class="node-cap"   x="{tx:.1}" y="{t3:.1}">{cap}</text>
    {ports}
  </g>
"#,
        w = NODE_W,
        h = NODE_H,
        tx = x + 16.0,
        t1 = y + 30.0,
        t2 = y + 52.0,
        t3 = y + 72.0,
        key = esc(&node.key),
        module = esc(&node.module),
        version = esc(&node.version),
        cap = esc(&node.capability),
    )
}

fn external_pill(x: f64, y: f64, contract: &str, id: &str) -> String {
    format!(
        r#"  <g class="ext">
    <rect x="{x:.1}" y="{y:.1}" width="{w}" height="{h}" rx="10"/>
    <text class="ext-title" x="{tx:.1}" y="{t1:.1}">input · {contract}</text>
    <text class="ext-id"    x="{tx:.1}" y="{t2:.1}">{id}</text>
  </g>
"#,
        w = EXT_W,
        h = EXT_H,
        tx = x + 14.0,
        t1 = y + 22.0,
        t2 = y + 40.0,
        contract = esc(contract.rsplit('.').next().unwrap_or(contract)),
        id = esc(&short(id)),
    )
}

fn answer_pill(x: f64, y: f64, id: &str) -> String {
    format!(
        r#"  <g class="answer">
    <rect x="{x:.1}" y="{y:.1}" width="{w}" height="{h}" rx="10"/>
    <text class="answer-title" x="{tx:.1}" y="{t1:.1}">terminal answer</text>
    <text class="answer-id"    x="{tx:.1}" y="{t2:.1}">{id}</text>
  </g>
"#,
        w = PILL_W,
        h = PILL_H,
        tx = x + 14.0,
        t1 = y + 20.0,
        t2 = y + 38.0,
        id = esc(&short(id)),
    )
}

fn page(graph: &Graph, chain: &[LedgerEntry], verified: bool, diagram: &str) -> String {
    let rows = chain
        .iter()
        .map(|e| {
            let subject = if e.subject.starts_with("sha256:") {
                short(&e.subject)
            } else {
                e.subject.clone()
            };
            format!(
                "<tr><td class=\"seq\">{}</td><td><code>{}</code></td><td class=\"subj\">{}</td><td class=\"hash\">{}…</td></tr>",
                e.seq,
                esc(&e.kind),
                esc(&subject),
                esc(&e.hash[..e.hash.len().min(16)]),
            )
        })
        .collect::<String>();

    let badge = if verified {
        r#"<span class="badge ok">✓ chain verifies</span>"#
    } else {
        r#"<span class="badge bad">✗ chain broken</span>"#
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>substrate — reconstructed dataflow</title>
<style>
  :root {{
    --bg:#f7f8fb; --panel:#ffffff; --ink:#1e1b2e; --muted:#6b7280; --line:#e5e7eb;
    --node:#eef2ff; --node-line:#6366f1; --node-ink:#1e1b4b;
    --ext:#f1f5f9; --ext-line:#94a3b8;
    --edge:#f59e0b; --edge-ink:#7c2d12;
    --answer:#ecfdf5; --answer-line:#10b981; --answer-ink:#064e3b;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg:#0d0f17; --panel:#151824; --ink:#e7e9f0; --muted:#8b93a7; --line:#242838;
      --node:#1e213a; --node-line:#818cf8; --node-ink:#c7d2fe;
      --ext:#1a1f2e; --ext-line:#64748b;
      --edge:#fbbf24; --edge-ink:#fde68a;
      --answer:#0f2a22; --answer-line:#34d399; --answer-ink:#a7f3d0;
    }}
  }}
  * {{ box-sizing:border-box; }}
  body {{ margin:0; background:var(--bg); color:var(--ink);
    font:15px/1.5 ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; }}
  .wrap {{ max-width:1100px; margin:0 auto; padding:40px 24px 80px; }}
  header h1 {{ margin:0 0 6px; font-size:26px; letter-spacing:-0.02em; }}
  header p {{ margin:0; color:var(--muted); max-width:70ch; }}
  .meta {{ display:flex; gap:10px; flex-wrap:wrap; align-items:center; margin:18px 0 4px; }}
  .badge {{ font-size:13px; font-weight:600; padding:4px 10px; border-radius:999px; }}
  .badge.ok {{ background:var(--answer); color:var(--answer-ink); border:1px solid var(--answer-line); }}
  .badge.bad {{ background:#fee2e2; color:#991b1b; border:1px solid #ef4444; }}
  .chip {{ font-size:13px; color:var(--muted); padding:4px 10px; border:1px solid var(--line); border-radius:999px; }}
  .panel {{ background:var(--panel); border:1px solid var(--line); border-radius:16px;
    padding:20px; margin-top:22px; }}
  .panel h2 {{ margin:0 0 4px; font-size:15px; text-transform:uppercase; letter-spacing:0.08em; color:var(--muted); }}
  .panel p.note {{ margin:0 0 14px; color:var(--muted); font-size:13.5px; }}
  .scroll {{ overflow-x:auto; }}
  svg {{ max-width:100%; height:auto; display:block; }}
  .edge {{ fill:none; stroke:var(--edge); stroke-width:2; opacity:0.85; }}
  .arrowhead {{ fill:var(--edge); }}
  .edge-label {{ fill:var(--edge-ink); font-size:11px; font-family:ui-monospace,monospace;
    paint-order:stroke; stroke:var(--panel); stroke-width:3px; }}
  .node rect {{ fill:var(--node); stroke:var(--node-line); stroke-width:1.5; }}
  .node-title {{ fill:var(--node-ink); font-weight:700; font-size:15px; }}
  .node-sub {{ fill:var(--node-line); font-size:12px; font-family:ui-monospace,monospace; }}
  .node-cap {{ fill:var(--muted); font-size:12px; font-family:ui-monospace,monospace; }}
  .port {{ fill:var(--node-line); }}
  .ext rect {{ fill:var(--ext); stroke:var(--ext-line); stroke-width:1.2; stroke-dasharray:4 3; }}
  .ext-title {{ fill:var(--muted); font-size:11px; }}
  .ext-id {{ fill:var(--ink); font-size:12px; font-family:ui-monospace,monospace; }}
  .answer rect {{ fill:var(--answer); stroke:var(--answer-line); stroke-width:1.5; }}
  .answer-title {{ fill:var(--answer-ink); font-size:11px; font-weight:700; }}
  .answer-id {{ fill:var(--answer-ink); font-size:12px; font-family:ui-monospace,monospace; }}
  table {{ width:100%; border-collapse:collapse; font-size:13.5px; }}
  th, td {{ text-align:left; padding:7px 10px; border-bottom:1px solid var(--line); }}
  th {{ color:var(--muted); font-weight:600; font-size:12px; text-transform:uppercase; letter-spacing:0.05em; }}
  td.seq {{ color:var(--muted); font-variant-numeric:tabular-nums; width:44px; }}
  td code {{ color:var(--node-line); font-family:ui-monospace,monospace; }}
  td.subj, td.hash {{ font-family:ui-monospace,monospace; color:var(--muted); }}
  footer {{ margin-top:28px; color:var(--muted); font-size:13px; }}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <h1>Reconstructed dataflow — <code>{run}</code></h1>
    <p>Every box and arrow below was rebuilt <strong>solely by decoding the append-only ledger</strong>
    — the same tamper-evident chain shown at the bottom. Nothing here reads live kernel state. That the
    picture reconstructs at all is the substrate's central guarantee: artifact refs are the data plane,
    and the chain records exactly what happened.</p>
  </header>

  <div class="meta">
    {badge}
    <span class="chip">run: {run} · {state}</span>
    <span class="chip">{nodes} nodes</span>
    <span class="chip">{entries} ledger entries</span>
  </div>

  <div class="panel">
    <h2>The flow</h2>
    <p class="note">dashed = external run input · amber = a typed Artifact flowing along a binding · green = the one terminal output that closed the run</p>
    <div class="scroll">
{diagram}
    </div>
  </div>

  <div class="panel">
    <h2>The ledger it was rebuilt from</h2>
    <p class="note">append-only, hash-chained; each entry commits to the previous entry's hash</p>
    <div class="scroll">
      <table>
        <thead><tr><th>#</th><th>kind</th><th>subject</th><th>hash</th></tr></thead>
        <tbody>{rows}</tbody>
      </table>
    </div>
  </div>

  <footer>Generated by <code>flow-example</code> on the srcport-substrate Rust SDK. See <code>SPEC.md</code>.</footer>
</div>
</body>
</html>"#,
        run = esc(&graph.run_id),
        state = esc(&graph.run_state),
        nodes = graph.nodes.len(),
        entries = chain.len(),
        badge = badge,
        diagram = diagram,
        rows = rows,
    )
}
