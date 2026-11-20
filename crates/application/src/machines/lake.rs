//! The lake's gate (0066, point 10): RustFS checks no more than "the
//! certificate is of the CA", so a Traefik in front of it, on each lake
//! node, requires the client certificate and asks the API, through
//! `forwardAuth`, whether each request may pass
//! ([`MachinesService::check_lake_for`]). Two machines are in a check: the
//! gate that asks, known by its own certificate, and the machine at its
//! door, which the gate names. The answer is the machines', never the
//! request's: the method and the path Traefik forwards are not looked at.
//!
//! A check comes with every request to the lake, thousands per job, so a
//! positive answer is kept for [`LAKE_CHECK_TTL`] by the certificate's
//! subject and serial, the gate's and the machine's each, and a pass is
//! not audited (a refusal is). Negative answers are not kept: a machine
//! approved, or presenting its next certificate, passes at once.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use auth_gateway::MachineRole::{App, Lake, LakeReplica, TrinoCoordinator, TrinoWorker, Worker};
use auth_gateway::{MachineCertificate, MachineRole};
use domain::AuditEventType::CertificateRefused;

use super::{MachinesService, Origin};

/// How long a pass is kept without asking PostgreSQL again. The price: a
/// machine retired, drained out or whose certificate was replaced keeps the
/// lake for at most this long after the change.
pub const LAKE_CHECK_TTL: Duration = Duration::from_secs(5);

/// The roles whose machines read or write the lake: the workers (DuckDB),
/// Trino's coordinator and workers (Iceberg), the lake's own machines, its
/// replicas (the user's ruling of 2026-11-11) and the application VM: every
/// role there is today. A role added later is refused until it is named here.
pub const LAKE_ROLES: [MachineRole; 6] = [
    Worker,
    TrinoCoordinator,
    TrinoWorker,
    Lake,
    LakeReplica,
    App,
];

/// The roles whose machines are a gate of the lake: who may ask for another.
pub const LAKE_GATE_ROLES: [MachineRole; 2] = [Lake, LakeReplica];

/// Why a request may not reach the lake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LakeCheckError {
    /// Not an approved or draining machine of a role in [`LAKE_ROLES`],
    /// presenting its serial in use or its next one; or asked by one that
    /// is not a gate, or for no one.
    Refused,
    Backend(String),
}

/// A certificate as the cache knows it: `OU`, `CN` and serial (lowercase).
/// The CA signs each serial once, and Traefik checked the signature.
type Key = (Option<String>, Option<String>, String);

/// The passes given, each until when it holds.
#[derive(Debug, Default)]
pub(super) struct LakePasses(Mutex<HashMap<Key, SystemTime>>);

impl LakePasses {
    fn key(presented: &MachineCertificate) -> Key {
        (
            presented.unit.clone(),
            presented.common_name.clone(),
            presented.serial.to_ascii_lowercase(),
        )
    }

    /// Whether `key` holds a pass at `now`; one that ends further than the
    /// TTL away (the clock went back) does not.
    fn holds(&self, key: &Key, now: SystemTime) -> bool {
        let passes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        passes.get(key).is_some_and(|until| {
            until
                .duration_since(now)
                .is_ok_and(|left| !left.is_zero() && left <= LAKE_CHECK_TTL)
        })
    }

    /// Keeps a pass for `key` from `now`, and drops the ones that ended.
    fn give(&self, key: Key, now: SystemTime) {
        let mut passes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        passes.retain(|_, until| *until > now);
        passes.insert(key, now + LAKE_CHECK_TTL);
    }
}

impl MachinesService {
    /// Whether the gate of `asker` may let the machine it names, `named`,
    /// reach the lake. The gate first: an approved or draining machine of
    /// a role in [`LAKE_GATE_ROLES`] presenting its serial in use or its
    /// next one, by [`Self::caller`], its pass kept as a machine's is.
    /// Anyone else is refused whatever it names, as is a gate that names no
    /// one: what names the machine is a header its sender wrote, believed
    /// only from a gate. Then the machine, by [`Self::check_lake`].
    pub async fn check_lake_for(
        &self,
        asker: &MachineCertificate,
        named: Option<&MachineCertificate>,
        origin: &Origin,
    ) -> Result<(), LakeCheckError> {
        let key = LakePasses::key(asker);
        if !self.lake_gates.holds(&key, self.clock.now()) {
            let role = asker
                .unit
                .as_deref()
                .and_then(MachineRole::parse)
                .filter(|role| LAKE_GATE_ROLES.contains(role));
            let gate = match role {
                Some(role) => self
                    .caller(asker, role)
                    .await
                    .map_err(|e| LakeCheckError::Backend(format!("{e:?}")))?,
                None => None,
            };
            if gate.is_none() {
                let name = asker.common_name.as_deref();
                self.audit_hold(CertificateRefused, name, "lake:gate", origin)
                    .await;
                return Err(LakeCheckError::Refused);
            }
            self.lake_gates.give(key, self.clock.now());
        }
        let Some(named) = named else {
            let name = asker.common_name.as_deref();
            self.audit_hold(CertificateRefused, name, "lake:unnamed", origin)
                .await;
            return Err(LakeCheckError::Refused);
        };
        self.check_lake(named, origin).await
    }

    /// Whether the machine of `presented` may reach the lake: a pass kept
    /// from the last [`LAKE_CHECK_TTL`], or else [`Self::caller`] with the
    /// role of its `OU` if that is one of [`LAKE_ROLES`] (approved or
    /// draining, the serial in use or the next one, which, first presented,
    /// becomes the one in use, as on every machine route). A pass is not
    /// audited, as there is one per request; a refusal is, and is not kept.
    pub async fn check_lake(
        &self,
        presented: &MachineCertificate,
        origin: &Origin,
    ) -> Result<(), LakeCheckError> {
        let key = LakePasses::key(presented);
        if self.lake.holds(&key, self.clock.now()) {
            return Ok(());
        }
        let role = presented
            .unit
            .as_deref()
            .and_then(MachineRole::parse)
            .filter(|role| LAKE_ROLES.contains(role));
        let machine = match role {
            Some(role) => self
                .caller(presented, role)
                .await
                .map_err(|e| LakeCheckError::Backend(format!("{e:?}")))?,
            None => None,
        };
        if machine.is_none() {
            let name = presented.common_name.as_deref();
            self.audit_hold(CertificateRefused, name, "lake:certificate", origin)
                .await;
            return Err(LakeCheckError::Refused);
        }
        self.lake.give(key, self.clock.now());
        Ok(())
    }
}
