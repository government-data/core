//! A lake's pools (decision 0029, points 5 and 6; 0066 step 11, the user's
//! rulings of 2026-11-14): a person defines a pool in Settings → Lake
//! before any of its nodes exists, by its name, its nodes' names in order
//! and how many drives each node has. Each node then joins with a token
//! that names it, and every node is given the same layout, derived here
//! alone ([`PoolLayout::volumes`]).
//!
//! All pools are one RustFS deployment: the lake's root keys and admin keys
//! are the lake's, kept under `pool:main` (ruling 2). An installation's lake
//! is either today's single node or pools, the first pool named
//! [`MAIN_POOL`](crate::domain::machine_install::MAIN_POOL) (ruling 3).

use std::time::SystemTime;

use crate::domain::machine::is_label;
use crate::domain::{AdminId, RepositoryError};

/// The fewest servers a pool has (0029, point 5).
pub const MIN_POOL_NODES: usize = 4;

/// The most: a bound against a mistaken request, not a limit of RustFS's
/// that was measured (four nodes were verified, 0066 step 11).
pub const MAX_POOL_NODES: usize = 16;

/// The most drives a node has, as for the replica (`MAX_DRIVES`).
pub const MAX_POOL_DRIVES: u16 = 16;

/// RustFS's port for its peers alone on a pool node (ruling 4); the gate
/// keeps 9000 for everyone else. 9010 was verified.
pub const RUSTFS_PEER_PORT: u16 = 9010;

/// The name every pool node maps to its own loopback, where the
/// replication's proxy listens (ruling 5, verified on 2026-11-14: RustFS
/// refuses `127.0.0.1` and `localhost` as a remote target). Not a node's.
pub const REPLICATION_ALIAS: &str = "qd-lake-replication";

/// A pool's layout, checked: a name, at least [`MIN_POOL_NODES`] nodes,
/// each a name a certificate and DNS take, and the drives of each node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolLayout {
    name: String,
    nodes: Vec<String>,
    drives: u16,
}

/// Why a pool's layout was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolRefusal {
    /// The pool's name is not a label.
    Name,
    /// Fewer than [`MIN_POOL_NODES`] nodes.
    TooFewNodes,
    /// More than [`MAX_POOL_NODES`].
    TooManyNodes,
    /// A node's name is not a host's name (lowercase letters, digits and
    /// hyphens, 1 to 63, not starting or ending with a hyphen), or is
    /// [`REPLICATION_ALIAS`].
    NodeName,
    /// The same node twice.
    RepeatedNode,
    /// Drives out of 1 to [`MAX_POOL_DRIVES`].
    Drives,
}

/// Whether `name` can be a pool node's: its certificate's CN and only DNS
/// name, and a name its peers resolve.
pub fn is_host_name(name: &str) -> bool {
    is_label(name) && !name.starts_with('-') && !name.ends_with('-')
}

impl PoolLayout {
    /// `nodes` trimmed, blanks ignored, in the order given.
    pub fn parse(name: &str, nodes: &[String], drives: u16) -> Result<Self, PoolRefusal> {
        if !is_host_name(name) {
            return Err(PoolRefusal::Name);
        }
        let given: Vec<&str> = nodes
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
            .collect();
        if given.len() < MIN_POOL_NODES {
            return Err(PoolRefusal::TooFewNodes);
        }
        if given.len() > MAX_POOL_NODES {
            return Err(PoolRefusal::TooManyNodes);
        }
        let mut checked: Vec<String> = Vec::with_capacity(given.len());
        for node in given {
            if !is_host_name(node) || node == REPLICATION_ALIAS {
                return Err(PoolRefusal::NodeName);
            }
            if checked.iter().any(|n| n == node) {
                return Err(PoolRefusal::RepeatedNode);
            }
            checked.push(node.to_string());
        }
        if !(1..=MAX_POOL_DRIVES).contains(&drives) {
            return Err(PoolRefusal::Drives);
        }
        Ok(Self {
            name: name.to_string(),
            nodes: checked,
            drives,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn nodes(&self) -> &[String] {
        &self.nodes
    }

    pub fn drives(&self) -> u16 {
        self.drives
    }

    /// RustFS's `RUSTFS_VOLUMES`, the same on every node: each node's
    /// drives, in order, at `/data/<n>` (as `join.sh` mounts them), on the
    /// peers' port, space-separated. A plain list of unrelated names was
    /// verified with RustFS 1.0.5 (0066 step 11).
    pub fn volumes(&self) -> String {
        let mut volumes = Vec::with_capacity(self.nodes.len() * usize::from(self.drives));
        for node in &self.nodes {
            for n in 1..=self.drives {
                volumes.push(format!("https://{node}:{RUSTFS_PEER_PORT}/data/{n}"));
            }
        }
        volumes.join(" ")
    }
}

/// A pool as kept, and who defined it last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LakePool {
    pub layout: PoolLayout,
    pub defined_by: AdminId,
    pub defined_at: SystemTime,
}

/// Port: the lake's pools, in the current tenant.
#[async_trait::async_trait]
pub trait LakePoolStore: Send + Sync {
    /// Keeps `layout`, in place of the pool of the same name if there is one.
    async fn define(
        &self,
        layout: &PoolLayout,
        by: &AdminId,
        at: SystemTime,
    ) -> Result<LakePool, RepositoryError>;

    async fn find(&self, name: &str) -> Result<Option<LakePool>, RepositoryError>;

    /// Every pool, by name.
    async fn all(&self) -> Result<Vec<LakePool>, RepositoryError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_pool_takes_four_host_names_or_more_in_order() {
        let given = names(&[" lake-b ", "lake-a", "", "lake-d", "lake-c"]);
        let pool = PoolLayout::parse("main", &given, 2).unwrap();
        assert_eq!(pool.nodes(), ["lake-b", "lake-a", "lake-d", "lake-c"]);
        assert_eq!((pool.name(), pool.drives()), ("main", 2));
        let sixteen: Vec<String> = (1..=16).map(|i| format!("n{i}")).collect();
        assert!(PoolLayout::parse("main", &sixteen, MAX_POOL_DRIVES).is_ok());
    }

    #[test]
    fn a_pool_out_of_form_is_refused() {
        let four = names(&["a", "b", "c", "d"]);
        let refused = |name: &str, nodes: &[&str], drives| {
            PoolLayout::parse(name, &names(nodes), drives).unwrap_err()
        };
        assert_eq!(refused("Main", &["a", "b", "c", "d"], 1), PoolRefusal::Name);
        assert_eq!(
            refused("main", &["a", "b", "c"], 1),
            PoolRefusal::TooFewNodes
        );
        let seventeen: Vec<String> = (1..=17).map(|i| format!("n{i}")).collect();
        let too_many = PoolLayout::parse("main", &seventeen, 1).unwrap_err();
        assert_eq!(too_many, PoolRefusal::TooManyNodes);
        for bad in [
            "-a",
            "a-",
            "A",
            "a.b",
            "a_b",
            "a b",
            "qd-lake-replication",
            "$(x)",
        ] {
            let nodes = [bad, "b", "c", "d"];
            assert_eq!(refused("main", &nodes, 1), PoolRefusal::NodeName, "{bad}");
        }
        let long = "a".repeat(64);
        assert_eq!(
            refused("main", &[&long, "b", "c", "d"], 1),
            PoolRefusal::NodeName
        );
        assert_eq!(
            refused("main", &["a", "b", "a", "d"], 1),
            PoolRefusal::RepeatedNode
        );
        for drives in [0, MAX_POOL_DRIVES + 1] {
            let pool = PoolLayout::parse("main", &four, drives);
            assert_eq!(pool.unwrap_err(), PoolRefusal::Drives);
        }
    }

    #[test]
    fn every_node_gets_one_volumes_string_of_each_node_and_drive() {
        let nodes = names(&["lake-a", "lake-b", "lake-c", "lake-d"]);
        let one = PoolLayout::parse("main", &nodes, 1).unwrap();
        assert_eq!(
            one.volumes(),
            "https://lake-a:9010/data/1 https://lake-b:9010/data/1 \
             https://lake-c:9010/data/1 https://lake-d:9010/data/1"
        );
        let two = PoolLayout::parse("main", &nodes, 2).unwrap().volumes();
        let parts: Vec<&str> = two.split(' ').collect();
        assert_eq!(parts.len(), 8);
        assert_eq!(
            &parts[..2],
            ["https://lake-a:9010/data/1", "https://lake-a:9010/data/2"]
        );
        assert_eq!(parts[7], "https://lake-d:9010/data/2");
    }
}
