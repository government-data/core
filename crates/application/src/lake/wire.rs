//! The ports' types as `lake-admin/1` writes them (`contracts::lake_admin`),
//! both ways: the generic client and an adapter speak the wire, the core
//! the ports.

use contracts::lake_admin as wire;

use super::{
    LakeCredentialKind, LakeError, LakeHealth, LakeKeys, ManagedAccess, ManagedPolicy,
    ReplicaTarget, ReplicationState, ReplicationSync, RetentionRules, SessionPolicy, WarehouseMade,
};

impl From<&RetentionRules> for wire::RetentionRules {
    fn from(r: &RetentionRules) -> Self {
        Self {
            versioning: r.versioning,
            locked_days: r.locked_days,
            noncurrent_expire_days: r.noncurrent_expire_days,
        }
    }
}

impl From<wire::RetentionRules> for RetentionRules {
    fn from(r: wire::RetentionRules) -> Self {
        Self {
            versioning: r.versioning,
            locked_days: r.locked_days,
            noncurrent_expire_days: r.noncurrent_expire_days,
        }
    }
}

impl From<&ReplicaTarget> for wire::ReplicaTarget {
    fn from(r: &ReplicaTarget) -> Self {
        Self {
            endpoint: r.endpoint.clone(),
            source_pool: r.source_pool,
        }
    }
}

impl From<wire::ReplicaTarget> for ReplicaTarget {
    fn from(r: wire::ReplicaTarget) -> Self {
        Self {
            endpoint: r.endpoint,
            source_pool: r.source_pool,
        }
    }
}

impl From<WarehouseMade> for wire::WarehouseOutcome {
    fn from(m: WarehouseMade) -> Self {
        match m {
            WarehouseMade::Made => Self::Made,
            WarehouseMade::Present => Self::Present,
            WarehouseMade::Failed { step, detail } => Self::Failed { step, detail },
        }
    }
}

impl From<wire::WarehouseOutcome> for WarehouseMade {
    fn from(m: wire::WarehouseOutcome) -> Self {
        match m {
            wire::WarehouseOutcome::Made => Self::Made,
            wire::WarehouseOutcome::Present => Self::Present,
            wire::WarehouseOutcome::Failed { step, detail } => Self::Failed { step, detail },
        }
    }
}

impl From<ReplicationState> for wire::ReplicationState {
    fn from(s: ReplicationState) -> Self {
        match s {
            ReplicationState::NotConfigured => Self::NotConfigured,
            ReplicationState::Configured => Self::Configured,
            ReplicationState::Failed { step, detail } => Self::Failed { step, detail },
        }
    }
}

impl From<wire::ReplicationState> for ReplicationState {
    fn from(s: wire::ReplicationState) -> Self {
        match s {
            wire::ReplicationState::NotConfigured => Self::NotConfigured,
            wire::ReplicationState::Configured => Self::Configured,
            wire::ReplicationState::Failed { step, detail } => Self::Failed { step, detail },
        }
    }
}

impl From<ReplicationSync> for wire::ReplicationSync {
    fn from(s: ReplicationSync) -> Self {
        Self {
            state: s.state.into(),
            pending: s.pending,
            failed: s.failed,
        }
    }
}

impl From<wire::ReplicationSync> for ReplicationSync {
    fn from(s: wire::ReplicationSync) -> Self {
        Self {
            state: s.state.into(),
            pending: s.pending,
            failed: s.failed,
        }
    }
}

impl From<&ManagedPolicy> for wire::ManagedPolicy {
    fn from(p: &ManagedPolicy) -> Self {
        let access = match &p.access {
            ManagedAccess::ReadUnder(prefix) => wire::ManagedAccess::ReadUnder(prefix.clone()),
            ManagedAccess::Read => wire::ManagedAccess::Read,
            ManagedAccess::ReadWrite => wire::ManagedAccess::ReadWrite,
            ManagedAccess::Administer => wire::ManagedAccess::Administer,
        };
        Self {
            name: p.name.clone(),
            access,
        }
    }
}

impl From<wire::ManagedPolicy> for ManagedPolicy {
    fn from(p: wire::ManagedPolicy) -> Self {
        let access = match p.access {
            wire::ManagedAccess::ReadUnder(prefix) => ManagedAccess::ReadUnder(prefix),
            wire::ManagedAccess::Read => ManagedAccess::Read,
            wire::ManagedAccess::ReadWrite => ManagedAccess::ReadWrite,
            wire::ManagedAccess::Administer => ManagedAccess::Administer,
        };
        Self {
            name: p.name,
            access,
        }
    }
}

impl From<SessionPolicy> for wire::SessionPolicy {
    fn from(p: SessionPolicy) -> Self {
        Self { policy: p.0 }
    }
}

impl From<wire::SessionPolicy> for SessionPolicy {
    fn from(p: wire::SessionPolicy) -> Self {
        Self(p.policy)
    }
}

impl From<LakeCredentialKind> for wire::CredentialKind {
    fn from(k: LakeCredentialKind) -> Self {
        match k {
            LakeCredentialKind::Root => Self::Root,
            LakeCredentialKind::Admin => Self::Admin,
            LakeCredentialKind::StsIssuer => Self::StsIssuer,
            LakeCredentialKind::QueryEngine => Self::QueryEngine,
        }
    }
}

impl From<wire::CredentialKind> for LakeCredentialKind {
    fn from(k: wire::CredentialKind) -> Self {
        match k {
            wire::CredentialKind::Root => Self::Root,
            wire::CredentialKind::Admin => Self::Admin,
            wire::CredentialKind::StsIssuer => Self::StsIssuer,
            wire::CredentialKind::QueryEngine => Self::QueryEngine,
        }
    }
}

impl From<&LakeKeys> for wire::Credentials {
    fn from(k: &LakeKeys) -> Self {
        Self {
            access_key: k.access_key.clone(),
            secret_key: k.secret_key.clone(),
        }
    }
}

impl From<wire::Credentials> for LakeKeys {
    fn from(k: wire::Credentials) -> Self {
        Self {
            access_key: k.access_key,
            secret_key: k.secret_key,
        }
    }
}

impl From<LakeHealth> for wire::Health {
    fn from(h: LakeHealth) -> Self {
        match h {
            LakeHealth::NoKeys => Self::NoKeys,
            LakeHealth::Answered {
                drives_online,
                drives_offline,
            } => Self::Answered {
                drives_online,
                drives_offline,
            },
            LakeHealth::KeysRefused { code } => Self::KeysRefused { code },
        }
    }
}

impl From<wire::Health> for LakeHealth {
    fn from(h: wire::Health) -> Self {
        match h {
            wire::Health::NoKeys => Self::NoKeys,
            wire::Health::Answered {
                drives_online,
                drives_offline,
            } => Self::Answered {
                drives_online,
                drives_offline,
            },
            wire::Health::KeysRefused { code } => Self::KeysRefused { code },
        }
    }
}

impl From<&LakeError> for wire::ErrorBody {
    fn from(e: &LakeError) -> Self {
        let (code, message) = match e {
            LakeError::Invalid(m) => (wire::ErrorCode::Invalid, m),
            LakeError::Refused(m) => (wire::ErrorCode::Refused, m),
            LakeError::Unknown(m) => (wire::ErrorCode::Unknown, m),
            LakeError::Conflict(m) => (wire::ErrorCode::Conflict, m),
            LakeError::Lake(m) => (wire::ErrorCode::Lake, m),
            LakeError::Unavailable(m) => (wire::ErrorCode::Unavailable, m),
        };
        Self::new(code, message.clone())
    }
}

impl From<wire::ErrorBody> for LakeError {
    fn from(e: wire::ErrorBody) -> Self {
        match e.code {
            wire::ErrorCode::Invalid => Self::Invalid(e.error),
            wire::ErrorCode::Refused => Self::Refused(e.error),
            wire::ErrorCode::Unknown => Self::Unknown(e.error),
            wire::ErrorCode::Conflict => Self::Conflict(e.error),
            wire::ErrorCode::Lake => Self::Lake(e.error),
            wire::ErrorCode::Unavailable => Self::Unavailable(e.error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_keeps_its_code_and_retry_both_ways() {
        for e in [
            LakeError::Invalid("a".into()),
            LakeError::Refused("b".into()),
            LakeError::Unknown("c".into()),
            LakeError::Conflict("d".into()),
            LakeError::Lake("e".into()),
            LakeError::Unavailable("f".into()),
        ] {
            let body = wire::ErrorBody::from(&e);
            assert_eq!(body.retryable, e.retryable());
            assert_eq!(LakeError::from(body), e);
        }
    }

    #[test]
    fn the_ports_types_survive_the_wire() {
        let rules = RetentionRules {
            versioning: true,
            locked_days: Some(30),
            noncurrent_expire_days: Some(7),
        };
        assert_eq!(
            RetentionRules::from(wire::RetentionRules::from(&rules)),
            rules
        );
        let policy = ManagedPolicy {
            name: "quality-viewer".into(),
            access: ManagedAccess::ReadUnder("reports/".into()),
        };
        assert_eq!(
            ManagedPolicy::from(wire::ManagedPolicy::from(&policy)),
            policy
        );
        let failed = ReplicationState::Failed {
            step: "rule".into(),
            detail: "x".into(),
        };
        assert_eq!(
            ReplicationState::from(wire::ReplicationState::from(failed.clone())),
            failed
        );
        for made in [
            WarehouseMade::Made,
            WarehouseMade::Present,
            WarehouseMade::Failed {
                step: "warehouse_tables".into(),
                detail: "HTTP 403".into(),
            },
        ] {
            let back = WarehouseMade::from(wire::WarehouseOutcome::from(made.clone()));
            assert_eq!(back, made);
        }
        for health in [
            LakeHealth::NoKeys,
            LakeHealth::Answered {
                drives_online: 3,
                drives_offline: 1,
            },
            LakeHealth::KeysRefused {
                code: "AccessDenied".into(),
            },
        ] {
            assert_eq!(LakeHealth::from(wire::Health::from(health.clone())), health);
        }
        let target = ReplicaTarget {
            endpoint: "lake-replica-1".into(),
            source_pool: true,
        };
        assert_eq!(
            ReplicaTarget::from(wire::ReplicaTarget::from(&target)),
            target
        );
        for kind in [
            LakeCredentialKind::Root,
            LakeCredentialKind::Admin,
            LakeCredentialKind::StsIssuer,
            LakeCredentialKind::QueryEngine,
        ] {
            assert_eq!(
                LakeCredentialKind::from(wire::CredentialKind::from(kind)),
                kind
            );
        }
    }
}
