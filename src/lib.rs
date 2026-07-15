use petgraph::algo;
use petgraph::graph::{DiGraph, NodeIndex};
use pgrx::prelude::*;
use std::collections::{HashMap, VecDeque};

pgrx::pg_module_magic!();

extension_sql_file!("../sql/load_order.sql", bootstrap);

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

/// Brandes' algorithm for unweighted directed graphs.
fn betweenness_centrality<N, E>(graph: &DiGraph<N, E>) -> HashMap<NodeIndex, f64>
where
    N: std::fmt::Debug,
{
    let n = graph.node_count();
    if n <= 1 {
        return graph.node_indices().map(|v| (v, 0.0)).collect();
    }

    let mut cb: HashMap<NodeIndex, f64> = graph.node_indices().map(|v| (v, 0.0)).collect();

    for s in graph.node_indices() {
        let mut stack = Vec::new();
        let mut pred: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
        let mut sigma: HashMap<NodeIndex, usize> = HashMap::new();
        let mut dist: HashMap<NodeIndex, isize> = HashMap::new();
        let mut queue = VecDeque::new();

        for v in graph.node_indices() {
            pred.insert(v, Vec::new());
            sigma.insert(v, 0);
            dist.insert(v, -1);
        }

        sigma.insert(s, 1);
        dist.insert(s, 0);
        queue.push_back(s);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            let dv = *dist.get(&v).unwrap_or(&-1);
            for w in graph.neighbors(v) {
                let dw = dist.get(&w).copied().unwrap_or(-1);
                if dw < 0 {
                    dist.insert(w, dv + 1);
                    queue.push_back(w);
                }
                if dw == dv + 1 {
                    *sigma.get_mut(&w).unwrap() += sigma.get(&v).copied().unwrap_or(0);
                    pred.get_mut(&w).unwrap().push(v);
                }
            }
        }

        let mut delta: HashMap<NodeIndex, f64> = HashMap::new();
        for v in graph.node_indices() {
            delta.insert(v, 0.0);
        }

        while let Some(w) = stack.pop() {
            let sw = *sigma.get(&w).unwrap_or(&1) as f64;
            for v in pred.get(&w).unwrap_or(&Vec::new()) {
                let sv = *sigma.get(v).unwrap_or(&1) as f64;
                *delta.get_mut(v).unwrap() +=
                    (sv / sw) * (1.0 + delta.get(&w).copied().unwrap_or(0.0));
            }
            if w != s {
                *cb.get_mut(&w).unwrap() += delta.get(&w).copied().unwrap_or(0.0);
            }
        }
    }

    // Normalize: divide by 2 for undirected; for directed, divide by (n-1)(n-2)
    // We normalize by (n-1)*(n-2) for directed graphs
    if n > 2 {
        let norm = ((n - 1) * (n - 2)) as f64;
        for val in cb.values_mut() {
            *val /= norm;
        }
    }

    cb
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

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_pagerank() {
        let sources = vec![1i64, 2, 3, 2, 4];
        let targets = vec![2i64, 3, 1, 4, 1];

        let results = crate::pagerank(
            PgArray::from(sources.as_slice()),
            PgArray::from(targets.as_slice()),
            Some(0.85),
            Some(100),
        );

        let rows: Vec<(i64, f64)> = results.collect();
        assert!(!rows.is_empty(), "PageRank should return results");
        assert!(rows.iter().any(|(id, _)| *id == 1));
    }

    #[pg_test]
    fn test_scc() {
        let sources = vec![1i64, 2, 3];
        let targets = vec![2i64, 3, 1];

        let results = crate::scc(
            PgArray::from(sources.as_slice()),
            PgArray::from(targets.as_slice()),
        );

        let rows: Vec<(i64, i64)> = results.collect();
        for (_id, comp) in &rows {
            assert_eq!(*comp, 0, "cycle nodes should be in same SCC");
        }
    }

    #[pg_test]
    fn test_betweenness() {
        let sources = vec![1i64, 1, 1];
        let targets = vec![2i64, 3, 4];

        let results = crate::betweenness(
            PgArray::from(sources.as_slice()),
            PgArray::from(targets.as_slice()),
        );

        let rows: Vec<(i64, f64)> = results.collect();
        let best = rows.first().map(|(id, _)| *id);
        assert_eq!(best, Some(1), "star center should have highest betweenness");
    }
}
