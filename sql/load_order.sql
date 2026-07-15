-- pg_petgraph: Graph algorithms for PostgreSQL

-- PageRank centrality
CREATE OR REPLACE FUNCTION pagerank(
    sources bigint[],
    targets bigint[],
    damping double precision DEFAULT 0.85,
    max_iter integer DEFAULT 100
) RETURNS TABLE(node_id bigint, score double precision)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'pagerank_wrapper';

-- Strongly Connected Components (Kosaraju)
CREATE OR REPLACE FUNCTION scc(
    sources bigint[],
    targets bigint[]
) RETURNS TABLE(node_id bigint, component_id bigint)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'scc_wrapper';

-- Betweenness Centrality (Brandes)
CREATE OR REPLACE FUNCTION betweenness(
    sources bigint[],
    targets bigint[]
) RETURNS TABLE(node_id bigint, centrality double precision)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'betweenness_wrapper';

-- Dijkstra shortest path
CREATE OR REPLACE FUNCTION dijkstra(
    sources bigint[],
    targets bigint[],
    start_node bigint
) RETURNS TABLE(node_id bigint, distance double precision)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'dijkstra_wrapper';

-- Topological sort
CREATE OR REPLACE FUNCTION toposort(
    sources bigint[],
    targets bigint[]
) RETURNS TABLE(position integer, node_id bigint)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'toposort_wrapper';

-- Cycle detection
CREATE OR REPLACE FUNCTION is_cyclic(
    sources bigint[],
    targets bigint[]
) RETURNS boolean
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'is_cyclic_wrapper';

-- Connected components (weakly)
CREATE OR REPLACE FUNCTION connected_components(
    sources bigint[],
    targets bigint[]
) RETURNS TABLE(node_id bigint, component_id bigint)
STRICT LANGUAGE c AS 'MODULE_PATHNAME', 'connected_components_wrapper';
