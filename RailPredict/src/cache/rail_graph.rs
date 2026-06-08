//! Snaps live train routes onto the baked GB rail network so paths follow real
//! track instead of cutting straight across country.
//!
//! The same `stations.json` (node coords) + `edges.json` (rail links) that draw
//! the grey network backdrop are loaded once into an adjacency list. A live
//! train's calling points are mapped to their nearest node, and consecutive
//! nodes are joined by the shortest path through the graph — so a London →
//! Edinburgh express curves through the real route rather than drawing a straight
//! line over the North Sea. The network is static, so resolved node-paths are
//! memoised and shared across every train that runs the same segment.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::LazyLock;

use dashmap::DashMap;

static STATIONS_JSON: &str = include_str!("../export/assets/stations.json");
static EDGES_JSON: &str = include_str!("../export/assets/edges.json");

struct Graph {
    /// `[lon,lat]` per node, indexed as in `stations.json`.
    coords: Vec<[f64; 2]>,
    /// Adjacency: node → list of `(neighbour, great-circle km)`.
    adj: Vec<Vec<(u32, f64)>>,
}

static GRAPH: LazyLock<Graph> = LazyLock::new(build_graph);

/// Memoised node-index shortest paths, keyed by `(from, to)`.
static PATH_CACHE: LazyLock<DashMap<(u32, u32), Vec<u32>>> = LazyLock::new(DashMap::new);

/// Nearest-node cache, keyed by coordinate quantised to ~100 m, so a station
/// shared by many services is resolved once.
static NODE_CACHE: LazyLock<DashMap<(i32, i32), u32>> = LazyLock::new(DashMap::new);

fn haversine(a: [f64; 2], b: [f64; 2]) -> f64 {
    let r = 6371.0_f64;
    let (lo1, la1) = (a[0].to_radians(), a[1].to_radians());
    let (lo2, la2) = (b[0].to_radians(), b[1].to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

fn build_graph() -> Graph {
    let coords: Vec<[f64; 2]> = serde_json::from_str(STATIONS_JSON).unwrap_or_default();
    // edges.json rows are [i, j, bucket, count]; only i/j matter here.
    let edges: Vec<[u32; 4]> = serde_json::from_str(EDGES_JSON).unwrap_or_default();
    let mut adj: Vec<Vec<(u32, f64)>> = vec![Vec::new(); coords.len()];
    for e in &edges {
        let (i, j) = (e[0] as usize, e[1] as usize);
        if i >= coords.len() || j >= coords.len() || i == j {
            continue;
        }
        let w = haversine(coords[i], coords[j]);
        adj[i].push((j as u32, w));
        adj[j].push((i as u32, w));
    }
    Graph { coords, adj }
}

fn nearest_node(g: &Graph, p: [f64; 2]) -> Option<u32> {
    let key = ((p[0] * 1000.0) as i32, (p[1] * 1000.0) as i32);
    if let Some(n) = NODE_CACHE.get(&key) {
        return Some(*n);
    }
    let mut best: Option<u32> = None;
    let mut best_d = f64::INFINITY;
    for (i, c) in g.coords.iter().enumerate() {
        let d = haversine(*c, p);
        if d < best_d {
            best_d = d;
            best = Some(i as u32);
        }
    }
    if let Some(n) = best {
        NODE_CACHE.insert(key, n);
    }
    best
}

// A min-heap entry for Dijkstra (BinaryHeap is a max-heap, so ordering is reversed).
struct HeapItem {
    node: u32,
    cost: f64,
}
impl PartialEq for HeapItem {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost
    }
}
impl Eq for HeapItem {}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for HeapItem {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}

/// Shortest path (inclusive node sequence) from `a` to `b`, memoised. Returns an
/// empty vec when `b` is unreachable.
fn shortest_path(g: &Graph, a: u32, b: u32) -> Vec<u32> {
    if a == b {
        return vec![a];
    }
    if let Some(p) = PATH_CACHE.get(&(a, b)) {
        return p.clone();
    }
    let n = g.coords.len();
    let mut dist = vec![f64::INFINITY; n];
    let mut prev = vec![u32::MAX; n];
    dist[a as usize] = 0.0;
    let mut heap = BinaryHeap::new();
    heap.push(HeapItem { node: a, cost: 0.0 });
    while let Some(HeapItem { node, cost }) = heap.pop() {
        if node == b {
            break;
        }
        if cost > dist[node as usize] {
            continue;
        }
        for &(nb, w) in &g.adj[node as usize] {
            let nd = cost + w;
            if nd < dist[nb as usize] {
                dist[nb as usize] = nd;
                prev[nb as usize] = node;
                heap.push(HeapItem { node: nb, cost: nd });
            }
        }
    }
    let mut path = Vec::new();
    if prev[b as usize] != u32::MAX {
        let mut cur = b;
        while cur != u32::MAX {
            path.push(cur);
            if cur == a {
                break;
            }
            cur = prev[cur as usize];
        }
        path.reverse();
        if path.first() != Some(&a) {
            path.clear();
        }
    }
    PATH_CACHE.insert((a, b), path.clone());
    path
}

/// Densify a calling-point route so it follows the rail network: map each point
/// to its nearest node and join consecutive nodes by the graph shortest path.
/// Falls back to the original points whenever nodes/paths can't be resolved, so a
/// route is never lost.
pub fn snap_route(calls: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let g = &*GRAPH;
    if g.coords.is_empty() || calls.len() < 2 {
        return calls.to_vec();
    }
    let mut nodes: Vec<u32> = Vec::with_capacity(calls.len());
    for c in calls {
        if let Some(node) = nearest_node(g, *c)
            && nodes.last() != Some(&node)
        {
            nodes.push(node);
        }
    }
    if nodes.len() < 2 {
        return calls.to_vec();
    }
    let mut out: Vec<u32> = vec![nodes[0]];
    for w in nodes.windows(2) {
        let seg = shortest_path(g, w[0], w[1]);
        if seg.len() >= 2 {
            out.extend_from_slice(&seg[1..]);
        } else {
            out.push(w[1]); // unreachable hop — keep the straight line rather than drop it
        }
    }
    out.iter().map(|&node| g.coords[node as usize]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_loads_and_has_nodes_and_edges() {
        let g = &*GRAPH;
        assert_eq!(g.coords.len(), 2751, "all stations loaded");
        let degree: usize = g.adj.iter().map(|a| a.len()).sum();
        assert!(degree > 0, "edges produced adjacency");
    }

    #[test]
    fn snap_densifies_a_long_hop() {
        // Two far-apart points should be joined by more than just the endpoints
        // once routed through the network (real track has intermediate nodes).
        let london = [-0.12768, 51.50739]; // ~Charing Cross
        let edinburgh = [-3.18827, 55.95206]; // ~Edinburgh Waverley
        let snapped = snap_route(&[london, edinburgh]);
        assert!(
            snapped.len() > 2,
            "a London→Edinburgh hop should gain intermediate rail nodes, got {}",
            snapped.len()
        );
    }

    #[test]
    fn snap_preserves_short_routes_without_loss() {
        let a = [-3.44308, 51.71506];
        let b = [-3.30056, 56.05459];
        let out = snap_route(&[a, b]);
        assert!(out.len() >= 2, "never returns fewer than the endpoints");
    }
}
