//! What a pool's node is installed with besides its compose file (0029,
//! point 5; 0066 step 11, the user's rulings 1, 4 and 5 of 2026-11-14): the
//! pool's layout as the application derives it, the same `RUSTFS_VOLUMES`
//! for every node, given to whoever presents a usable join token of the
//! role `lake`, which names the node, as the compose file is. Nothing
//! secret: the root keys are their own route.

use auth_gateway::{MachineRole, PoolLayout};

use super::{InstallFileError, MachinesService};

/// The port of the replica's gate: the one port a lake machine publishes
/// (lake-rustfs 0.4.0, decision 0091, ruling 71).
const REPLICA_GATE_PORT: u16 = 443;

impl MachinesService {
    /// The environment file a pool's node reads (`pool.env`): `QD_POOL`,
    /// `QD_POOL_NODE` (the node the token names), `QD_POOL_NODES` (every
    /// node in order, for `join.sh` to check their names resolve),
    /// `QD_POOL_DRIVES`, `RUSTFS_VOLUMES`, and `QD_LAKE_REPLICA_GATE` when
    /// one replica is approved, for the replication's proxy.
    pub async fn role_pool_env(
        &self,
        token: &str,
        role: MachineRole,
    ) -> Result<String, InstallFileError> {
        let (_, grant) = self.grant(token, role).await?;
        let (Some(pool), Some(node)) = (grant.pool, grant.node) else {
            return Err(InstallFileError::NotFound);
        };
        let Some(store) = &self.pools else {
            return Err(InstallFileError::Unavailable);
        };
        let backend = |e: String| InstallFileError::Backend(e);
        let found = store
            .find(&pool)
            .await
            .map_err(|e| backend(e.to_string()))?;
        let Some(defined) = found else {
            return Err(InstallFileError::NotFound);
        };
        let replica = self.replica_machine().await;
        let replica = replica.map_err(|e| backend(format!("{e:?}")))?.ok();
        Ok(pool_env(&defined.layout, &node, replica.as_deref()))
    }
}

/// The file itself: one `NAME=value` a line, no value with a space but
/// the lists, which `join.sh` reads whole.
fn pool_env(layout: &PoolLayout, node: &str, replica: Option<&str>) -> String {
    let mut env = format!(
        "QD_POOL={}\nQD_POOL_NODE={node}\nQD_POOL_NODES=\"{}\"\nQD_POOL_DRIVES={}\n\
         RUSTFS_VOLUMES=\"{}\"\n",
        layout.name(),
        layout.nodes().join(" "),
        layout.drives(),
        layout.volumes(),
    );
    if let Some(replica) = replica {
        env.push_str(&format!(
            "QD_LAKE_REPLICA_GATE={replica}:{REPLICA_GATE_PORT}\n"
        ));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_names_the_node_the_layout_and_the_replica_when_known() {
        let nodes: Vec<String> = ["lake-a", "lake-b", "lake-c", "lake-d"]
            .map(String::from)
            .to_vec();
        let layout = PoolLayout::parse("main", &nodes, 1).unwrap();
        let env = pool_env(&layout, "lake-c", Some("replica-1"));
        assert_eq!(
            env,
            "QD_POOL=main\nQD_POOL_NODE=lake-c\nQD_POOL_NODES=\"lake-a lake-b lake-c lake-d\"\n\
             QD_POOL_DRIVES=1\nRUSTFS_VOLUMES=\"https://lake-a:9010/data/1 \
             https://lake-b:9010/data/1 https://lake-c:9010/data/1 \
             https://lake-d:9010/data/1\"\nQD_LAKE_REPLICA_GATE=replica-1:443\n"
        );
        assert!(!pool_env(&layout, "lake-c", None).contains("REPLICA"));
    }
}
