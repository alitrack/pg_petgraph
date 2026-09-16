# pg_petgraph

[![CI](https://github.com/alitrack/pg_petgraph/actions/workflows/ci.yml/badge.svg)](https://github.com/alitrack/pg_petgraph/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![PostgreSQL 15 | 16 | 17](https://img.shields.io/badge/PostgreSQL-15%20%7C%2016%20%7C%2017-blue)](https://www.postgresql.org/)

给 PostgreSQL 装上一组图算法，用 Rust 写，底层是 [pgrx](https://github.com/pgcentralfoundation/pgrx) + [petgraph](https://github.com/petgraph/petgraph)。

**不引入新的查询语言、不建图命名空间、不加存储层**——把两个 `bigint[]`（起点数组、终点数组）交给一个 SQL 函数，直接返回结果表。10 个算法，覆盖中心性、连通性、社区发现、拓扑排序与环检测。

```sql
CREATE EXTENSION pg_petgraph;

SELECT * FROM pagerank(ARRAY[1,2,3,2,4], ARRAY[2,3,1,4,1]);
-- 返回列: node_id bigint, score double precision
```

## 函数清单

所有函数接收 `sources bigint[]`、`targets bigint[]`，描述一个**无权有向图**的边，返回一张表。

| 函数 | 签名 | 返回 |
|---|---|---|
| `pagerank` | `pagerank(sources, targets, damping => 0.85, max_iter => 100)` | `(node_id, score)` |
| `scc` | `scc(sources, targets)` | `(node_id, component_id)` — Kosaraju |
| `connected_components` | `connected_components(sources, targets)` | `(node_id, component_id)` — 弱连通 |
| `betweenness` | `betweenness(sources, targets)` | `(node_id, centrality)` — Brandes |
| `closeness` | `closeness(sources, targets)` | `(node_id, centrality)` |
| `eigenvector` | `eigenvector(sources, targets, max_iter => 100, tolerance => 1e-6)` | `(node_id, centrality)` |
| `louvain` | `louvain(sources, targets)` | `(node_id, community_id)` |
| `dijkstra` | `dijkstra(sources, targets, start_node)` | `(node_id, distance)` — 单源 |
| `toposort` | `toposort(sources, targets)` | `(position, node_id)` |
| `is_cyclic` | `is_cyclic(sources, targets)` | `boolean` |

用法：

```sql
-- PageRank
SELECT * FROM pagerank(ARRAY[1,2,3,2,4], ARRAY[2,3,1,4,1], damping => 0.85, max_iter => 100);

-- 强连通分量
SELECT * FROM scc(ARRAY[1,2,3], ARRAY[2,3,1]);

-- 介数中心性
SELECT * FROM betweenness(ARRAY[1,1,1], ARRAY[2,3,4]);

-- 单源最短路
SELECT * FROM dijkstra(ARRAY[1,1,2], ARRAY[2,3,3], 1);

-- 环检测，可当 WHERE 条件用
SELECT is_cyclic(ARRAY[1,2], ARRAY[2,1]);

-- 社区发现
SELECT * FROM louvain(ARRAY[1,1,2,3], ARRAY[2,3,3,4]);
```

## 为什么再做一个图扩展

| 项目 | 路线 | 为什么不能替代 |
|---|---|---|
| [Apache AGE](https://github.com/apache/age) | 把 openCypher 查询语言 + 图命名空间装进 PostgreSQL | 项目活跃，但你要采纳 Cypher 与图对象；它是图数据库那一层，不是一组纯 SQL 函数 |
| [Apache MADlib](https://github.com/apache/madlib) | 完整机器学习套件 | 图算法只是大依赖里的一小块，不是图专用 |
| [pgRouting](https://github.com/pgRouting/pgrouting) | 路网路径规划 | 重心在拓扑寻路，不在中心性 / 社区发现 |

pg_petgraph 走的是纯 SQL 路线：两个数组进，一张表出，不新增语言、不新增存储层。面向的场景是——边已经躺在你的表里，你希望在产生它的那条 `SELECT` 里顺手拿到中心性或社区编号。

## 工作原理

```mermaid
flowchart LR
    A["SQL: pagerank(ARRAY[src], ARRAY[tgt])"] --> B["pgrx #[pg_extern] 包装"]
    B --> C["用边数组构建 petgraph::DiGraph"]
    C --> D["算法（保留原节点 id）"]
    D --> E["TableIterator → 结果表"]
```

节点 id 是 `bigint`，输出时原样保留；只需要边（孤立节点无法表达，因为它不出现在任一数组里）。

## 安装

### 源码安装

需要 Rust 1.96+ 以及 PostgreSQL 15/16/17 的开发环境。

```bash
cargo install cargo-pgrx --version 0.13.1 --locked
cargo pgrx init --pg17 /usr/bin/pg_config

git clone https://github.com/alitrack/pg_petgraph.git
cd pg_petgraph
cargo pgrx install --release --pg-config /usr/bin/pg_config
```

然后在 psql 里：

```sql
CREATE EXTENSION pg_petgraph;
```

### 支持版本

Cargo feature：`pg15` / `pg16` / `pg17`（默认 `pg17`）。CI 在 **PostgreSQL 16 与 17** 上跑 `cargo pgrx test`；`pg15` 提供了 feature，但不在 CI 覆盖范围内。

## 边界与限制

- **无权有向图**——暂不支持边的权重参数。
- **全内存**——每次调用会把整份边表物化；适合能舒服放进内存的图，不是十亿边的磁盘图方案。
- **全图函数**——每次调用都在完整边集上重算，没有增量或预计算索引。
- 不是路由引擎——`dijkstra` 返回单源距离；没有 A*、转向限制或路网拓扑处理。

## 姊妹项目

[`duckdb_petgraph`](https://github.com/alitrack/duckdb_petgraph) —— 同样的思路做给 DuckDB。图算法不该要求你先有个图数据库，无论你当下坐在哪一个里。

## 许可证

MIT，见 [LICENSE](LICENSE)。
