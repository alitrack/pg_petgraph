# pg_petgraph

PostgreSQL extension providing graph algorithms via [pgrx](https://github.com/pgcentralfoundation/pgrx) and [petgraph](https://github.com/petgraph/petgraph).

## Algorithms

| Function | Description |
|----------|-------------|
| `pagerank` | PageRank centrality with configurable damping, iterations, tolerance |
| `scc` | Kosaraju's strongly connected components |
| `betweenness` | Betweenness centrality |

## Why

PostgreSQL ecosystem lacks lightweight built-in graph algorithms:
- AGE is in maintenance mode
- Madlib is heavy (full ML suite)
- pgRouting focuses on pathfinding, not centrality/community detection

pg_petgraph fills this gap with a small, focused extension.

## Installation

```sql
CREATE EXTENSION pg_petgraph;
```

## Usage

```sql
-- PageRank on edges (source, target)
SELECT * FROM pagerank(
    ARRAY[1, 2, 3, 2, 4],  -- sources
    ARRAY[2, 3, 1, 4, 1],  -- targets
    damping => 0.85,
    max_iter => 100
);

-- Strongly connected components
SELECT * FROM scc(
    ARRAY[1, 2, 3],
    ARRAY[2, 3, 1]
);

-- Betweenness centrality
SELECT * FROM betweenness(
    ARRAY[1, 1, 1],
    ARRAY[2, 3, 4]
);
```

## Build

Requires Rust 1.96+, pgrx, PostgreSQL 15-17 development headers.

```bash
cargo pgrx init --pg17 /usr/bin/pg_config
cargo build --release
cargo pgrx run pg17   # interactive psql with extension loaded
```

## Algorithms (planned)

| Function | Status | Description |
|----------|--------|-------------|
| `pagerank` | ✅ | PageRank centrality |
| `scc` | ✅ | Kosaraju strongly connected components |
| `betweenness` | ✅ | Betweenness centrality (Brandes) |
| `dijkstra` | ✅ | Single-source shortest path |
| `closeness` | 🔜 | Closeness centrality |
| `toposort` | ✅ | Topological sort |
| `is_cyclic` | ✅ | Cycle detection |
| `connected_components` | ✅ | Weakly connected components |
| `eigenvector` | 🔜 | Eigenvector centrality |
| `louvain` | 🔜 | Community detection |

## License

MIT
