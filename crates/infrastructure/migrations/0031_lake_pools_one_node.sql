-- A lake may start on one node (decision 0091, point 3): a pool has 1 to 16
-- nodes. Its drives in all are checked by the application (a first pool of
-- one node and one drive, or at least two drives in all).
ALTER TABLE public.lake_pools DROP CONSTRAINT lake_pools_nodes_check,
    ADD CONSTRAINT lake_pools_nodes_check CHECK (cardinality(nodes) BETWEEN 1 AND 16);
