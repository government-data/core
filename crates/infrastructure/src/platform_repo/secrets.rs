//! Global secrets: stored encrypted, never returned with their values.

use super::*;

const SECRET_SELECT: &str = "
    SELECT s.id::text AS id, s.name, s.description, s.updated_at, s.kind, s.meta
      FROM secrets s";

fn row_secret(r: &PgRow) -> R<Secret> {
    Ok(Secret {
        id: SecretId::new(r.try_get::<String, _>("id").map_err(db)?),
        name: r.try_get("name").map_err(db)?,
        description: r.try_get("description").map_err(db)?,
        updated_at: r.try_get("updated_at").map_err(db)?,
        kind: r.try_get("kind").map_err(db)?,
        meta: r.try_get("meta").map_err(db)?,
    })
}

impl PgPlatformRepository {
    /// Those whose names `scope` allows: each part of the scope is one more
    /// condition of the `WHERE` (see [`iam::like`]).
    pub(super) async fn list_secrets(&self, scope: &iam::Scope) -> R<Vec<Secret>> {
        let mut conditions = vec!["true".to_string()];
        let mut binds: Vec<Vec<String>> = Vec::new();
        for part in &scope.parts {
            let allow: Vec<String> = part.allow.iter().map(|p| iam::like(p)).collect();
            let deny: Vec<String> = part.deny.iter().map(|p| iam::like(p)).collect();
            binds.push(allow);
            let a = binds.len();
            binds.push(deny);
            let d = binds.len();
            conditions.push(format!(
                "({} OR s.name LIKE ANY(${a})) AND NOT (s.name LIKE ANY(${d}))",
                part.all
            ));
        }
        let sql = format!(
            "{SECRET_SELECT} WHERE {} ORDER BY s.name",
            conditions.join(" AND ")
        );
        let mut query = sqlx::query(&sql);
        for b in &binds {
            query = query.bind(b);
        }
        query
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .iter()
            .map(row_secret)
            .collect()
    }

    pub(super) async fn create_secret(&self, secret: &NewSecret) -> R<Secret> {
        let id: String = sqlx::query_scalar(
            "INSERT INTO secrets (name, description, value, kind, meta)
             VALUES ($1, $2, $3, $4, $5) RETURNING id::text",
        )
        .bind(&secret.name)
        .bind(&secret.description)
        .bind(&secret.sealed)
        .bind(&secret.kind)
        .bind(&secret.meta)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match db(e) {
            RepositoryError::Conflict(_) => {
                RepositoryError::Conflict("there is already a secret with that name".into())
            }
            other => other,
        })?;
        let row = sqlx::query(&format!("{SECRET_SELECT} WHERE s.id = $1::uuid"))
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        row_secret(&row)
    }

    pub(super) async fn update_secret(
        &self,
        id: &SecretId,
        description: Option<&str>,
        sealed: Option<(&str, &Value)>,
    ) -> R<Secret> {
        let done = sqlx::query(
            "UPDATE secrets
                SET description = COALESCE($2, description),
                    value = COALESCE($3, value),
                    meta = COALESCE($4, meta),
                    updated_at = now()
              WHERE id = $1::uuid",
        )
        .bind(id.as_str())
        .bind(description)
        .bind(sealed.map(|(s, _)| s))
        .bind(sealed.map(|(_, m)| m))
        .execute(&self.pool)
        .await
        .map_err(db)?;
        affected(done.rows_affected())?;
        let row = sqlx::query(&format!("{SECRET_SELECT} WHERE s.id = $1::uuid"))
            .bind(id.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        row_secret(&row)
    }

    pub(super) async fn delete_secret(&self, id: &SecretId) -> R<()> {
        let done = sqlx::query("DELETE FROM secrets WHERE id = $1::uuid")
            .bind(id.as_str())
            .execute(&self.pool)
            .await
            .map_err(db)?;
        affected(done.rows_affected())
    }

    pub(super) async fn secret_values(&self, ids: &[SecretId]) -> R<HashMap<SecretId, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<&str> = ids.iter().map(SecretId::as_str).collect();
        // An id that is not a UUID is just not there (instead of failing the whole query).
        let rows = sqlx::query(
            "SELECT id::text AS id, value FROM secrets
              WHERE id::text = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| {
                Ok((
                    SecretId::new(r.try_get::<String, _>("id").map_err(db)?),
                    r.try_get::<String, _>("value").map_err(db)?,
                ))
            })
            .collect()
    }
}
