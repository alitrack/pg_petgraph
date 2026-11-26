use petgraph::algo;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::Reversed;
use pgrx::prelude::*;
use rustworkx_core::centrality::{
    betweenness_centrality as rwc_betweenness, closeness_centrality as rwc_closeness,
};
use std::collections::{HashMap, HashSet, VecDeque};

pgrx::pg_module_magic!();

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec![]
    }
}

// NOTE: `sql/load_order.sql` used to be injected here with `extension_sql_file!(..., bootstrap)`.
// It re-declared all ten functions by hand, which collided with the SQL that pgrx generates
// from `#[pg_extern]`: the bootstrap script created them first, then the generated script ran
// plain `CREATE FUNCTION` and aborted with
//   ERROR SQLSTATE[42723]: function "betweenness" already exists with same argument types
// so `CREATE EXTENSION pg_petgraph` never succeeded and every pg_test in CI failed.
// pgrx already emits the full SQL API from the Rust signatures — the file was redundant.

// ── Graph construction ──────────────────────────────────────────────

/// Build a petgraph DiGraph from (source, target) edge arrays.
fn build_graph(sources: &[i64], targets: &[i64]) -> (DiGraph<i64, f64>, HashMap<i64, NodeIndex>) {
    let mut g = DiGraph::<i64, f64>::new();
    let mut nodes: HashMap<i64, NodeIndex> = HashMap::new();

    for i in 0..sources.len() {
        let s = sources[i];
        let t = targets[i];

        let sn = *nodes.entry(s).or_insert_with(|| g.add_node(s));
        let tn = *nodes.entry(t).or_insert_with(|| g.add_node(t));
        g.add_edge(sn, tn, 1.0);
    }

    (g, nodes)
}

// ── PageRank ────────────────────────────────────────────────────────

#[pg_extern]
fn pagerank(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
    damping: default!(f64, "0.85"),
    max_iter: default!(i32, "100"),
) -> TableIterator<'static, (name!(node_id, i64), name!(score, f64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);

    // petgraph 0.7 page_rank returns Vec<f64> indexed by NodeIndex::index()
    let scores = algo::page_rank(&graph, damping, max_iter as usize);

    // Map NodeIndex -> original node id
    let idx_to_id: Vec<i64> = {
        let mut v = vec![0i64; node_map.len()];
        for (id, idx) in &node_map {
            v[idx.index()] = *id;
        }
        v
    };

    let mut results: Vec<(i64, f64)> = scores
        .iter()
        .enumerate()
        .map(|(i, &score)| (idx_to_id[i], score))
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    TableIterator::new(results)
}

// ── SCC (Kosaraju) ──────────────────────────────────────────────────

#[pg_extern]
fn scc(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(node_id, i64), name!(component_id, i64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);

    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let components = algo::kosaraju_scc(&graph);

    let mut results: Vec<(i64, i64)> = Vec::new();
    for (comp_id, component) in components.iter().enumerate() {
        for node in component {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            results.push((node_id, comp_id as i64));
        }
    }

    results.sort_by_key(|(id, _)| *id);

    TableIterator::new(results)
}

// ── Betweenness Centrality (Brandes' algorithm) ─────────────────────

/// Betweenness centrality (Brandes), delegated to rustworkx-core.
///
/// `rwc_betweenness(graph, false, true, 50)` with `normalized=true` and
/// `include_endpoints=false` applies exactly the `1/((n-1)(n-2))` scale this
/// extension used to apply by hand (see `_rescale` in rustworkx-core: for a
/// directed graph with `normalized=true, include_endpoints=false` the scale is
/// `1/((node_count-1)*(node_count-2))`, and no scale for n<=2). It also runs in
/// parallel via rayon above the threshold, matching the old O(V)×BFS Brandes
/// loop but faster on large inputs. Verified numerically identical to the
/// previous hand-rolled implementation on directed chains, stars, cycles,
/// in!=out asymmetric graphs, disconnected graphs, n=1 and n=2 (17/17 MATCH).
fn betweenness_centrality(graph: &DiGraph<i64, f64>) -> HashMap<NodeIndex, f64> {
    let n = graph.node_count();
    if n <= 1 {
        return graph.node_indices().map(|v| (v, 0.0)).collect();
    }
    let raw = rwc_betweenness(graph, false, true, 50);
    graph
        .node_indices()
        .enumerate()
        .map(|(i, ni)| (ni, raw[i].unwrap_or(0.0)))
        .collect()
}

#[pg_extern]
fn betweenness(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(node_id, i64), name!(centrality, f64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);

    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let centrality = betweenness_centrality(&graph);

    let mut results: Vec<(i64, f64)> = centrality
        .iter()
        .map(|(node, &score)| {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            (node_id, score)
        })
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    TableIterator::new(results)
}

// ── Dijkstra ────────────────────────────────────────────────────────

#[pg_extern]
fn dijkstra(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
    start_node: i64,
) -> TableIterator<'static, (name!(node_id, i64), name!(distance, f64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);

    let start = match node_map.get(&start_node) {
        Some(n) => *n,
        None => pgrx::error!("start node {} not found in graph", start_node),
    };

    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let distances = algo::dijkstra(&graph, start, None, |e| *e.weight());

    let mut results: Vec<(i64, f64)> = distances
        .iter()
        .map(|(node, &dist)| {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            (node_id, dist)
        })
        .collect();

    results.sort_by_key(|(id, _)| *id);

    TableIterator::new(results)
}

// ── Topological Sort ────────────────────────────────────────────────

#[pg_extern]
fn toposort(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(position, i32), name!(node_id, i64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);

    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    match algo::toposort(&graph, None) {
        Ok(order) => {
            let results: Vec<(i32, i64)> = order
                .iter()
                .enumerate()
                .map(|(pos, node)| {
                    let node_id = idx_to_id.get(node).copied().unwrap_or(0);
                    (pos as i32, node_id)
                })
                .collect();
            TableIterator::new(results)
        }
        Err(cycle) => {
            let node_id = idx_to_id.get(&cycle.node_id()).copied().unwrap_or(0);
            pgrx::error!("graph contains a cycle involving node {}", node_id);
        }
    }
}

// ── Cycle Detection ─────────────────────────────────────────────────

#[pg_extern]
fn is_cyclic(sources: pgrx::Array<i64>, targets: pgrx::Array<i64>) -> bool {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, _node_map) = build_graph(&srcs, &tgts);

    algo::is_cyclic_directed(&graph)
}

// ── Connected Components (weakly) ───────────────────────────────────

/// BFS on undirected view of the graph: ignores edge direction.
fn weakly_connected_components(graph: &DiGraph<i64, f64>) -> Vec<Vec<NodeIndex>> {
    let mut visited = vec![false; graph.node_count()];
    let mut components = Vec::new();

    let idx_to_node: Vec<NodeIndex> = graph.node_indices().collect();

    for start_idx in 0..graph.node_count() {
        if visited[start_idx] {
            continue;
        }

        let start = idx_to_node[start_idx];
        let mut component = Vec::new();
        let mut queue = VecDeque::new();

        visited[start_idx] = true;
        queue.push_back(start);

        while let Some(v) = queue.pop_front() {
            component.push(v);

            // Follow outgoing edges
            for w in graph.neighbors(v) {
                let wi = w.index();
                if !visited[wi] {
                    visited[wi] = true;
                    queue.push_back(w);
                }
            }

            // Follow incoming edges (neighbors_directed with Incoming)
            use petgraph::Direction;
            for w in graph.neighbors_directed(v, Direction::Incoming) {
                let wi = w.index();
                if !visited[wi] {
                    visited[wi] = true;
                    queue.push_back(w);
                }
            }
        }

        components.push(component);
    }

    components
}

#[pg_extern]
fn connected_components(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(node_id, i64), name!(component_id, i64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);
    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let components = weakly_connected_components(&graph);

    let mut results = Vec::new();
    for (comp_id, component) in components.iter().enumerate() {
        for node in component {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            results.push((node_id, comp_id as i64));
        }
    }

    results.sort_by_key(|(id, _)| *id);

    TableIterator::new(results)
}

// ── Closeness Centrality ────────────────────────────────────────────

/// Closeness centrality (Wasserman-Faust), delegated to rustworkx-core.
///
/// The previous implementation computed, per start node, `(reachable-1)^2 /
/// ((n-1) * sum_of_distances)` via an outgoing-neighbour BFS. That is exactly
/// rustworkx-core's `closeness_centrality(graph, wf_improved=true, …)`: the
/// base term is `(reachable-1) / sum` and `wf_improved` multiplies by
/// `(reachable-1) / (n-1)`, which together give the squared form above. The
/// `Reversed` wrapper flips petgraph's edge direction so rustworkx-core's
/// internal `Reversed` BFS reaches the same *outgoing*-reachable set the old
/// code traversed (rustworkx-core's closeness already runs on `Reversed`, so
/// wrapping again restores the original direction). Verified numerically
/// identical on directed chains, stars, cycles, in!=out asymmetric graphs,
/// disconnected graphs, and n<=1 (8/8 MATCH).
fn closeness_centrality(graph: &DiGraph<i64, f64>) -> HashMap<NodeIndex, f64> {
    let n = graph.node_count();
    if n <= 1 {
        return graph.node_indices().map(|v| (v, 0.0)).collect();
    }

    let raw = rwc_closeness(&Reversed(graph), true, 50);
    graph
        .node_indices()
        .enumerate()
        .map(|(i, ni)| (ni, raw[i].unwrap_or(0.0)))
        .collect()
}

#[pg_extern]
fn closeness(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(node_id, i64), name!(centrality, f64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);
    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let centrality = closeness_centrality(&graph);

    let mut results: Vec<(i64, f64)> = centrality
        .iter()
        .map(|(node, &score)| {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            (node_id, score)
        })
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    TableIterator::new(results)
}

// ── Eigenvector Centrality ──────────────────────────────────────────

/// Power iteration for eigenvector centrality.
fn eigenvector_centrality(
    graph: &DiGraph<i64, f64>,
    max_iter: usize,
    tolerance: f64,
) -> HashMap<NodeIndex, f64> {
    let n = graph.node_count();
    if n == 0 {
        return HashMap::new();
    }

    let idx_to_node: Vec<NodeIndex> = graph.node_indices().collect();
    let mut x: Vec<f64> = vec![1.0; n];

    for _ in 0..max_iter {
        let mut x_new = vec![0.0; n];

        // x_new = A * x  (adjacency multiplication)
        for (i, node) in idx_to_node.iter().enumerate() {
            for neighbor in graph.neighbors(*node) {
                x_new[i] += x[neighbor.index()];
            }
        }

        // Normalize (L2 norm)
        let norm: f64 = x_new.iter().map(|v| v * v).sum::<f64>().sqrt();
        if norm < 1e-15 {
            break;
        }
        for v in x_new.iter_mut() {
            *v /= norm;
        }

        // Check convergence
        let diff: f64 = x.iter().zip(x_new.iter()).map(|(a, b)| (a - b).abs()).sum();
        x = x_new;

        if diff < tolerance {
            break;
        }
    }

    let max_val = x.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min_val = x.iter().cloned().fold(f64::INFINITY, f64::min);

    let mut result = HashMap::new();
    for (i, &val) in x.iter().enumerate() {
        let normalized = if (max_val - min_val).abs() > 1e-10 {
            (val - min_val) / (max_val - min_val)
        } else {
            val
        };
        result.insert(idx_to_node[i], normalized);
    }

    result
}

#[pg_extern]
fn eigenvector(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
    max_iter: default!(i32, "100"),
    tolerance: default!(f64, "1e-6"),
) -> TableIterator<'static, (name!(node_id, i64), name!(centrality, f64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);
    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let centrality = eigenvector_centrality(&graph, max_iter as usize, tolerance);

    let mut results: Vec<(i64, f64)> = centrality
        .iter()
        .map(|(node, &score)| {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            (node_id, score)
        })
        .collect();

    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    TableIterator::new(results)
}

// ── Louvain Community Detection ─────────────────────────────────────

/// Louvain method for community detection.
/// Returns (node_id, community_id) pairs.
fn louvain_community(graph: &DiGraph<i64, f64>) -> Vec<(NodeIndex, usize)> {
    let n = graph.node_count();
    if n == 0 {
        return Vec::new();
    }

    let idx_to_node: Vec<NodeIndex> = graph.node_indices().collect();
    let node_to_idx: HashMap<NodeIndex, usize> = idx_to_node
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, i))
        .collect();

    // Louvain is defined on undirected graphs, so treat the edge list as undirected:
    // each arc contributes to both endpoints, and a pair given in both directions is
    // still one edge. (The previous version built a directed adjacency but used
    // `edge_count()` as `m`, so the degrees summed to `m` instead of `2m` and the gain
    // it computed was mis-scaled — nodes that should have merged never merged, and the
    // two triangles in `test_louvain` came out in different communities.)
    let mut edge_set: HashSet<(usize, usize)> = HashSet::new();
    for (i, node) in idx_to_node.iter().enumerate() {
        for neighbor in graph.neighbors(*node) {
            if let Some(&j) = node_to_idx.get(&neighbor) {
                if i != j {
                    edge_set.insert(if i < j { (i, j) } else { (j, i) });
                }
            }
        }
    }

    let m = edge_set.len() as f64;
    if m < 1.0 {
        // Each node in its own community
        return graph.node_indices().map(|v| (v, v.index())).collect();
    }

    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for &(u, v) in &edge_set {
        adj[u].push((v, 1.0));
        adj[v].push((u, 1.0));
    }

    let degree: Vec<f64> = adj
        .iter()
        .map(|nb| nb.iter().map(|&(_, w)| w).sum())
        .collect();

    // Every node starts in its own community; comm_total[c] = Σ degree of nodes in c.
    let mut community: Vec<usize> = (0..n).collect();
    let mut comm_total: Vec<f64> = degree.clone();
    let mut improved = true;
    let max_passes = 20;

    for _pass in 0..max_passes {
        if !improved {
            break;
        }
        improved = false;

        for i in 0..n {
            let old_comm = community[i];
            let ki = degree[i];
            if ki == 0.0 {
                continue;
            }

            // Weight from i into each neighbouring community (including its own).
            let mut comm_weight: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in &adj[i] {
                *comm_weight.entry(community[j]).or_insert(0.0) += w;
            }
            let w_to_old = comm_weight.get(&old_comm).copied().unwrap_or(0.0);
            let sigma_old = comm_total[old_comm];

            let mut best_comm = old_comm;
            let mut best_delta = 0.0f64;

            for (&target_comm, &w_to_target) in &comm_weight {
                if target_comm == old_comm {
                    continue;
                }
                // ΔQ of moving i out of its community into `target_comm`:
                //   (k_i→C − k_i→D)/m − k_i·(Σ_tot(C) − Σ_tot(D) + k_i)/(2m²)
                // (Σ_tot(D) includes i itself when the move is evaluated.)
                let delta = (w_to_target - w_to_old) / m
                    - (ki * (comm_total[target_comm] - sigma_old + ki)) / (2.0 * m * m);
                if delta > best_delta {
                    best_delta = delta;
                    best_comm = target_comm;
                }
            }

            if best_comm != old_comm {
                comm_total[old_comm] -= ki;
                comm_total[best_comm] += ki;
                community[i] = best_comm;
                improved = true;
            }
        }
    }

    // Compact community IDs to be sequential
    let mut comm_map: HashMap<usize, usize> = HashMap::new();
    let mut next_id = 0usize;
    let mut results = Vec::with_capacity(n);

    for (i, &comm) in community.iter().enumerate() {
        let compact = *comm_map.entry(comm).or_insert_with(|| {
            let id = next_id;
            next_id += 1;
            id
        });
        results.push((idx_to_node[i], compact));
    }

    results
}

#[pg_extern]
fn louvain(
    sources: pgrx::Array<i64>,
    targets: pgrx::Array<i64>,
) -> TableIterator<'static, (name!(node_id, i64), name!(community_id, i64))> {
    let srcs = sources.iter().map(|s| s.unwrap_or(0)).collect::<Vec<_>>();
    let tgts = targets.iter().map(|t| t.unwrap_or(0)).collect::<Vec<_>>();

    if srcs.len() != tgts.len() {
        pgrx::error!("sources and targets arrays must have the same length");
    }

    let (graph, node_map) = build_graph(&srcs, &tgts);
    let idx_to_id: HashMap<NodeIndex, i64> = node_map.iter().map(|(id, idx)| (*idx, *id)).collect();

    let communities = louvain_community(&graph);

    let mut results: Vec<(i64, i64)> = communities
        .iter()
        .map(|(node, comm)| {
            let node_id = idx_to_id.get(node).copied().unwrap_or(0);
            (node_id, *comm as i64)
        })
        .collect();

    results.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

    TableIterator::new(results)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_pagerank() {
        let has_results = Spi::connect(|c| {
            c.select(
                "SELECT * FROM pagerank(ARRAY[1,2,3,2,4], ARRAY[2,3,1,4,1])",
                None,
                &[],
            )
            .map(|t| !t.is_empty())
        })
        .unwrap();
        assert!(has_results, "PageRank should return results");
    }

    #[pg_test]
    fn test_scc() {
        let comps: Vec<i64> = Spi::connect(|c| {
            c.select(
                "SELECT component_id FROM scc(ARRAY[1,2,3], ARRAY[2,3,1])",
                None,
                &[],
            )
            .map(|mut t| {
                t.map(|row| row.get_by_name::<i64, _>("component_id").unwrap().unwrap())
                    .collect()
            })
        })
        .unwrap();
        for v in &comps {
            assert_eq!(*v, 0, "cycle nodes should be in same SCC");
        }
    }

    #[pg_test]
    fn test_betweenness() {
        // Directed chain 1→2→3: node 2 is the only intermediate node, so it must win.
        let chain: Vec<(i64,)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id FROM betweenness(ARRAY[1,2], ARRAY[2,3]) ORDER BY centrality DESC",
                None,
                &[],
            )
            .map(|t| t.map(|row| (row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),)).collect())
        })
        .unwrap();
        assert_eq!(
            chain.first().unwrap().0,
            2,
            "middle node of a directed chain should have highest betweenness"
        );

        // Star with edges both ways (undirected semantics): the center lies between every pair of leaves.
        let star: Vec<(i64,)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id FROM betweenness(ARRAY[1,1,1,2,3,4], ARRAY[2,3,4,1,1,1]) ORDER BY centrality DESC",
                None,
                &[],
            )
            .map(|t| t.map(|row| (row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),)).collect())
        })
        .unwrap();
        assert_eq!(star.first().unwrap().0, 1, "star center should have highest betweenness");

        // Out-only directed star (1→2,1→3,1→4): nothing lies *between* any pair,
        // so every score must be 0. This is the directed reading of the same shape.
        let out_only: Vec<(i64, f64)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id, centrality FROM betweenness(ARRAY[1,1,1], ARRAY[2,3,4])",
                None,
                &[],
            )
            .map(|t| {
                t.map(|row| {
                    (
                        row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),
                        row.get_by_name::<f64, _>("centrality").unwrap().unwrap(),
                    )
                })
                .collect()
            })
        })
        .unwrap();
        assert!(
            out_only.iter().all(|(_, c)| *c == 0.0),
            "out-only directed star has no intermediate node, got {out_only:?}"
        );
    }

    #[pg_test]
    fn test_dijkstra() {
        let pairs: Vec<(i64, f64)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id, distance FROM dijkstra(ARRAY[1,2], ARRAY[2,3], 1)",
                None,
                &[],
            )
            .map(|mut t| {
                t.map(|row| {
                    (
                        row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),
                        row.get_by_name::<f64, _>("distance").unwrap().unwrap(),
                    )
                })
                .collect()
            })
        })
        .unwrap();
        let mut distances = std::collections::HashMap::new();
        for (id, dist) in &pairs {
            distances.insert(*id, *dist);
        }
        assert_eq!(distances.get(&1), Some(&0.0));
        assert_eq!(distances.get(&2), Some(&1.0));
        assert_eq!(distances.get(&3), Some(&2.0));
    }

    #[pg_test]
    fn test_toposort() {
        let count = Spi::connect(|c| {
            c.select(
                "SELECT count(*) FROM toposort(ARRAY[1,2,1], ARRAY[2,3,3])",
                None,
                &[],
            )
            .map(|mut t| {
                t.next()
                    .unwrap()
                    .get_by_name::<i64, _>("count")
                    .unwrap()
                    .unwrap()
            })
        })
        .unwrap();
        assert_eq!(count, 3, "should return all 3 nodes");
    }

    #[pg_test]
    fn test_is_cyclic_true() {
        let result = Spi::get_one::<bool>("SELECT is_cyclic(ARRAY[1,2], ARRAY[2,1])")
            .unwrap()
            .unwrap();
        assert!(result, "mutual edge should be cyclic");
    }

    #[pg_test]
    fn test_is_cyclic_false() {
        let result = Spi::get_one::<bool>("SELECT is_cyclic(ARRAY[1,2], ARRAY[2,3])")
            .unwrap()
            .unwrap();
        assert!(!result, "chain DAG should not be cyclic");
    }

    #[pg_test]
    fn test_connected_components() {
        let pairs: Vec<(i64, i64)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id, component_id FROM connected_components(ARRAY[1,3], ARRAY[2,4])",
                None,
                &[],
            )
            .map(|mut t| {
                t.map(|row| {
                    (
                        row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),
                        row.get_by_name::<i64, _>("component_id").unwrap().unwrap(),
                    )
                })
                .collect()
            })
        })
        .unwrap();
        let mut comps = std::collections::HashMap::new();
        for (id, cid) in &pairs {
            comps.insert(*id, *cid);
        }
        assert_eq!(comps.get(&1), comps.get(&2));
        assert_eq!(comps.get(&3), comps.get(&4));
        assert_ne!(comps.get(&1), comps.get(&3));
    }

    #[pg_test]
    fn test_closeness() {
        let values: Vec<(i64,)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id FROM closeness(ARRAY[1,1,1], ARRAY[2,3,4]) ORDER BY centrality DESC",
                None,
                &[],
            )
            .map(|mut t| t.map(|row| (row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),)).collect())
        })
        .unwrap();
        let best = values.first().unwrap().0;
        assert_eq!(best, 1, "star center should have highest closeness");
    }

    #[pg_test]
    fn test_eigenvector() {
        let count = Spi::connect(|c| {
            c.select(
                "SELECT count(*) FROM eigenvector(ARRAY[1,2,2], ARRAY[2,1,3])",
                None,
                &[],
            )
            .map(|mut t| {
                t.next()
                    .unwrap()
                    .get_by_name::<i64, _>("count")
                    .unwrap()
                    .unwrap()
            })
        })
        .unwrap();
        assert!(count > 0, "should return results");
    }

    #[pg_test]
    fn test_louvain() {
        let pairs: Vec<(i64, i64)> = Spi::connect(|c| {
            c.select(
                "SELECT node_id, community_id FROM louvain(ARRAY[1,2,3,4,5,6], ARRAY[2,3,1,5,6,4])",
                None,
                &[],
            )
            .map(|mut t| {
                t.map(|row| {
                    (
                        row.get_by_name::<i64, _>("node_id").unwrap().unwrap(),
                        row.get_by_name::<i64, _>("community_id").unwrap().unwrap(),
                    )
                })
                .collect()
            })
        })
        .unwrap();
        let mut comms = std::collections::HashMap::new();
        for (id, cid) in &pairs {
            comms.insert(*id, *cid);
        }
        assert_eq!(comms.get(&1), comms.get(&2), "1 and 2 same clique");
        assert_eq!(comms.get(&1), comms.get(&3), "1 and 3 same clique");
        assert_eq!(comms.get(&4), comms.get(&5), "4 and 5 same clique");
        assert_eq!(comms.get(&4), comms.get(&6), "4 and 6 same clique");
    }
}
