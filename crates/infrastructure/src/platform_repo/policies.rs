//! IAM (ADR 0016): custom policies, attachments, groups and service accounts.

use std::collections::BTreeMap;

use ::iam::{Policy, Statement};
use platform::{
    Group, GroupFields, GroupMember, NewServiceAccount, PrincipalKind, ServiceAccount, UserAccess,
};

use super::*;

fn conflict_as(message: &'static str) -> impl Fn(sqlx::Error) -> RepositoryError {
    move |e| match db(e) {
        RepositoryError::Conflict(_) => RepositoryError::Conflict(message.into()),
        other => other,
    }
}

fn principal_table(kind: PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::User => "admin_users",
        PrincipalKind::Group => "iam_groups",
        PrincipalKind::ServiceAccount => "service_accounts",
    }
}

impl PgPlatformRepository {
    // --- Policies ------------------------------------------------------------

    pub(super) async fn list_policies(&self) -> R<Vec<Policy>> {
        let rows =
            sqlx::query("SELECT name, description, statements FROM iam_policies ORDER BY name")
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        rows.iter()
            .map(|r| {
                let statements: Value = r.try_get("statements").map_err(db)?;
                Ok(Policy {
                    name: r.try_get("name").map_err(db)?,
                    description: r.try_get("description").map_err(db)?,
                    statements: serde_json::from_value::<Vec<Statement>>(statements)
                        .map_err(|e| RepositoryError::Backend(format!("policy statements: {e}")))?,
                    managed: false,
                })
            })
            .collect()
    }

    pub(super) async fn put_policy(&self, policy: &Policy) -> R<()> {
        let statements = serde_json::to_value(&policy.statements)
            .map_err(|e| RepositoryError::Backend(e.to_string()))?;
        sqlx::query(
            "INSERT INTO iam_policies (name, description, statements) VALUES ($1, $2, $3)
             ON CONFLICT (tenant_id, name)
             DO UPDATE SET description = EXCLUDED.description,
                           statements = EXCLUDED.statements,
                           updated_at = now()",
        )
        .bind(&policy.name)
        .bind(&policy.description)
        .bind(statements)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub(super) async fn delete_policy(&self, name: &str) -> R<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("DELETE FROM iam_attachments WHERE policy = $1")
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let done = sqlx::query("DELETE FROM iam_policies WHERE name = $1")
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        affected(done.rows_affected())?;
        tx.commit().await.map_err(db)
    }

    pub(super) async fn policy_usage(&self) -> R<HashMap<String, i64>> {
        let rows = sqlx::query("SELECT policy, count(*) AS n FROM iam_attachments GROUP BY policy")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("policy").map_err(db)?,
                    r.try_get("n").map_err(db)?,
                ))
            })
            .collect()
    }

    pub(super) async fn attached(&self, kind: PrincipalKind, id: &str) -> R<Vec<String>> {
        sqlx::query_scalar(
            "SELECT policy FROM iam_attachments
              WHERE principal_kind = $1 AND principal_id = $2::uuid ORDER BY policy",
        )
        .bind(kind.as_str())
        .bind(id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)
    }

    pub(super) async fn attach(&self, kind: PrincipalKind, id: &str, policies: &[String]) -> R<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE id = $1::uuid)",
            principal_table(kind)
        ))
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db)?;
        if !exists {
            return Err(RepositoryError::NotFound);
        }
        sqlx::query(
            "DELETE FROM iam_attachments WHERE principal_kind = $1 AND principal_id = $2::uuid",
        )
        .bind(kind.as_str())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "INSERT INTO iam_attachments (principal_kind, principal_id, policy)
             SELECT $1, $2::uuid, p FROM unnest($3::text[]) AS p
             ON CONFLICT DO NOTHING",
        )
        .bind(kind.as_str())
        .bind(id)
        .bind(policies)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub(super) async fn account_policies(&self, admin_id: &str) -> R<Vec<String>> {
        sqlx::query_scalar(
            "SELECT policy FROM iam_attachments
              WHERE principal_kind = 'user' AND principal_id = $1::uuid
             UNION
             SELECT a.policy FROM iam_attachments a
               JOIN iam_group_members m
                 ON a.principal_kind = 'group' AND a.principal_id = m.group_id
              WHERE m.admin_id = $1::uuid
             ORDER BY 1",
        )
        .bind(admin_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)
    }

    pub(super) async fn user_access(&self) -> R<Vec<UserAccess>> {
        let rows = sqlx::query(
            "SELECT u.id::text AS id,
                    COALESCE((SELECT array_agg(a.policy ORDER BY a.policy) FROM iam_attachments a
                               WHERE a.principal_kind = 'user' AND a.principal_id = u.id), '{}')
                      AS policies,
                    COALESCE((SELECT array_agg(DISTINCT g.name) FROM iam_group_members m
                                JOIN iam_groups g ON g.id = m.group_id
                               WHERE m.admin_id = u.id), '{}') AS groups
               FROM admin_users u ORDER BY u.email",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| {
                Ok(UserAccess {
                    admin_id: r.try_get("id").map_err(db)?,
                    policies: r.try_get("policies").map_err(db)?,
                    groups: r.try_get("groups").map_err(db)?,
                })
            })
            .collect()
    }

    // --- Groups --------------------------------------------------------------

    pub(super) async fn list_groups(&self) -> R<Vec<Group>> {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        let rows = sqlx::query(
            "SELECT id::text AS id, name, description, directory_groups FROM iam_groups",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for r in &rows {
            let id: String = r.try_get("id").map_err(db)?;
            groups.insert(
                id.clone(),
                Group {
                    id,
                    name: r.try_get("name").map_err(db)?,
                    description: r.try_get("description").map_err(db)?,
                    directory_groups: r.try_get("directory_groups").map_err(db)?,
                    members: Vec::new(),
                    policies: Vec::new(),
                },
            );
        }
        let members = sqlx::query(
            "SELECT m.group_id::text AS group_id, u.id::text AS admin_id, u.email,
                    u.display_name, m.source
               FROM iam_group_members m JOIN admin_users u ON u.id = m.admin_id
              ORDER BY u.email, m.source",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for r in &members {
            let group: String = r.try_get("group_id").map_err(db)?;
            if let Some(g) = groups.get_mut(&group) {
                g.members.push(GroupMember {
                    admin_id: r.try_get("admin_id").map_err(db)?,
                    email: r.try_get("email").map_err(db)?,
                    display_name: r.try_get("display_name").map_err(db)?,
                    source: r.try_get("source").map_err(db)?,
                });
            }
        }
        let attached = sqlx::query(
            "SELECT principal_id::text AS id, policy FROM iam_attachments
              WHERE principal_kind = 'group' ORDER BY policy",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        for r in &attached {
            let group: String = r.try_get("id").map_err(db)?;
            if let Some(g) = groups.get_mut(&group) {
                g.policies.push(r.try_get("policy").map_err(db)?);
            }
        }
        let mut out: Vec<Group> = groups.into_values().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub(super) async fn create_group(&self, fields: &GroupFields) -> R<String> {
        sqlx::query_scalar(
            "INSERT INTO iam_groups (name, description, directory_groups)
             VALUES ($1, $2, $3) RETURNING id::text",
        )
        .bind(&fields.name)
        .bind(&fields.description)
        .bind(&fields.directory_groups)
        .fetch_one(&self.pool)
        .await
        .map_err(conflict_as("there is already a group with that name"))
    }

    pub(super) async fn update_group(&self, id: &str, fields: &GroupFields) -> R<()> {
        let done = sqlx::query(
            "UPDATE iam_groups SET name = $2, description = $3, directory_groups = $4,
                                   updated_at = now()
              WHERE id = $1::uuid",
        )
        .bind(id)
        .bind(&fields.name)
        .bind(&fields.description)
        .bind(&fields.directory_groups)
        .execute(&self.pool)
        .await
        .map_err(conflict_as("there is already a group with that name"))?;
        affected(done.rows_affected())
    }

    pub(super) async fn delete_group(&self, id: &str) -> R<()> {
        let done = sqlx::query("DELETE FROM iam_groups WHERE id = $1::uuid")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        affected(done.rows_affected())
    }

    pub(super) async fn set_group_members(&self, id: &str, admin_ids: &[String]) -> R<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM iam_groups WHERE id = $1::uuid)")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        if !exists {
            return Err(RepositoryError::NotFound);
        }
        sqlx::query(
            "DELETE FROM iam_group_members WHERE group_id = $1::uuid AND source = 'manual'",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "INSERT INTO iam_group_members (group_id, admin_id, source)
             SELECT $1::uuid, u.id, 'manual' FROM admin_users u
              WHERE u.id::text = ANY($2)
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(admin_ids)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    // --- Service accounts ------------------------------------------------------

    pub(super) async fn list_service_accounts(
        &self,
        owner: Option<&str>,
    ) -> R<Vec<ServiceAccount>> {
        let rows = sqlx::query(
            "SELECT s.id::text AS id, s.owner_id::text AS owner_id, u.email, s.name,
                    s.access_key, s.created_at, s.last_used_at, s.expires_at,
                    COALESCE((SELECT array_agg(a.policy ORDER BY a.policy) FROM iam_attachments a
                               WHERE a.principal_kind = 'service_account'
                                 AND a.principal_id = s.id), '{}') AS policies
               FROM service_accounts s JOIN admin_users u ON u.id = s.owner_id
              WHERE $1::text IS NULL OR s.owner_id::text = $1
              ORDER BY u.email, s.name",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| {
                Ok(ServiceAccount {
                    id: r.try_get("id").map_err(db)?,
                    owner_id: r.try_get("owner_id").map_err(db)?,
                    owner_email: r.try_get("email").map_err(db)?,
                    name: r.try_get("name").map_err(db)?,
                    access_key: r.try_get("access_key").map_err(db)?,
                    policies: r.try_get("policies").map_err(db)?,
                    created_at: r.try_get("created_at").map_err(db)?,
                    last_used_at: r.try_get("last_used_at").map_err(db)?,
                    expires_at: r.try_get("expires_at").map_err(db)?,
                })
            })
            .collect()
    }

    pub(super) async fn create_service_account(&self, account: &NewServiceAccount) -> R<String> {
        sqlx::query_scalar(
            "INSERT INTO service_accounts (owner_id, name, access_key, secret_hash, expires_at)
             VALUES ($1::uuid, $2, $3, $4, $5) RETURNING id::text",
        )
        .bind(&account.owner_id)
        .bind(&account.name)
        .bind(&account.access_key)
        .bind(&account.secret_hash)
        .bind(account.expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(conflict_as(
            "there is already a service account with that name",
        ))
    }

    pub(super) async fn delete_service_account(&self, id: &str, owner: Option<&str>) -> R<()> {
        let done = sqlx::query(
            "DELETE FROM service_accounts
              WHERE id = $1::uuid AND ($2::text IS NULL OR owner_id::text = $2)",
        )
        .bind(id)
        .bind(owner)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        affected(done.rows_affected())
    }
}
