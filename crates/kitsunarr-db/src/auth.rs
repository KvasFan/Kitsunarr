//! Users and sessions.

use sqlx::{FromRow, SqliteExecutor};

use crate::{Database, DbError, DbResult, now};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub role: String,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct SessionUser {
    pub session_id: i64,
    pub user_id: i64,
    pub username: String,
    pub expires_at: String,
    pub last_seen_at: String,
}

pub async fn user_count(ex: impl SqliteExecutor<'_>) -> DbResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(ex)
        .await?)
}

/// Creates the first administrator atomically: fails with `Conflict` if any user exists, so
/// two concurrent setup requests cannot both succeed.
pub async fn create_first_admin(
    db: &Database,
    username: &str,
    password_hash: &str,
) -> DbResult<i64> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    if count > 0 {
        return Err(DbError::Conflict("an administrator already exists".into()));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, role, created_at) VALUES (?, ?, 'admin', ?) RETURNING id",
    )
    .bind(username.trim())
    .bind(password_hash)
    .bind(now())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| DbError::from_write(e, "user"))?;
    tx.commit().await?;
    Ok(id)
}

pub async fn find_user_by_name(
    ex: impl SqliteExecutor<'_>,
    username: &str,
) -> DbResult<Option<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE username = ?")
        .bind(username.trim())
        .fetch_optional(ex)
        .await?)
}

pub async fn find_user(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<Option<User>> {
    Ok(sqlx::query_as("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(ex)
        .await?)
}

pub async fn set_password(
    ex: impl SqliteExecutor<'_>,
    user_id: i64,
    password_hash: &str,
) -> DbResult<()> {
    let r = sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(user_id)
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn touch_login(ex: impl SqliteExecutor<'_>, user_id: i64) -> DbResult<()> {
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(now())
        .bind(user_id)
        .execute(ex)
        .await?;
    Ok(())
}

pub struct NewSession<'a> {
    pub token_hash: &'a str,
    pub user_id: i64,
    pub expires_at: &'a str,
    pub user_agent: Option<&'a str>,
    pub remote_addr: Option<&'a str>,
}

pub async fn create_session(ex: impl SqliteExecutor<'_>, s: NewSession<'_>) -> DbResult<i64> {
    let ts = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at, user_agent, remote_addr) \
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(s.token_hash)
    .bind(s.user_id)
    .bind(&ts)
    .bind(s.expires_at)
    .bind(&ts)
    .bind(s.user_agent.map(|ua| ua.chars().take(256).collect::<String>()))
    .bind(s.remote_addr)
    .fetch_one(ex)
    .await?)
}

/// Resolves a live (unexpired) session by token hash.
pub async fn find_session(
    ex: impl SqliteExecutor<'_>,
    token_hash: &str,
    now_ts: &str,
) -> DbResult<Option<SessionUser>> {
    Ok(sqlx::query_as(
        "SELECT s.id AS session_id, u.id AS user_id, u.username, s.expires_at, s.last_seen_at \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(token_hash)
    .bind(now_ts)
    .fetch_optional(ex)
    .await?)
}

pub async fn touch_session(
    ex: impl SqliteExecutor<'_>,
    session_id: i64,
    expires_at: Option<&str>,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE sessions SET last_seen_at = ?, expires_at = COALESCE(?, expires_at) WHERE id = ?",
    )
    .bind(now())
    .bind(expires_at)
    .bind(session_id)
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn delete_session(ex: impl SqliteExecutor<'_>, token_hash: &str) -> DbResult<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash)
        .execute(ex)
        .await?;
    Ok(())
}

/// Revokes every session of a user except, optionally, one to keep.
pub async fn delete_user_sessions(
    ex: impl SqliteExecutor<'_>,
    user_id: i64,
    keep_session_id: Option<i64>,
) -> DbResult<u64> {
    let r = sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id IS NOT ?")
        .bind(user_id)
        .bind(keep_session_id)
        .execute(ex)
        .await?;
    Ok(r.rows_affected())
}

/// Removes expired sessions, password and OIDC (single-sign-on) ones.
pub async fn purge_expired_sessions(pool: &sqlx::SqlitePool) -> DbResult<u64> {
    let ts = now();
    let r = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(&ts)
        .execute(pool)
        .await?;
    let sso = sqlx::query("DELETE FROM sso_sessions WHERE expires_at <= ?")
        .bind(&ts)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() + sso.rows_affected())
}

/// The single administrator, if any (the account a password reset applies to).
pub async fn first_admin(ex: impl SqliteExecutor<'_>) -> DbResult<Option<User>> {
    Ok(
        sqlx::query_as("SELECT * FROM users ORDER BY (role = 'admin') DESC, id LIMIT 1")
            .fetch_optional(ex)
            .await?,
    )
}

/// Sonarr's `UserService.Upsert` for a password reset: the existing account gets the new name
/// and password hash; without an account one is created. Returns the account id.
pub async fn upsert_admin(db: &Database, username: &str, password_hash: &str) -> DbResult<i64> {
    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users ORDER BY (role = 'admin') DESC, id LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
    let id = match existing {
        Some(id) => {
            sqlx::query("UPDATE users SET username = ?, password_hash = ? WHERE id = ?")
                .bind(username.trim())
                .bind(password_hash)
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(|e| DbError::from_write(e, "user"))?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, role, created_at) VALUES (?, ?, 'admin', ?) RETURNING id",
        )
        .bind(username.trim())
        .bind(password_hash)
        .bind(now())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| DbError::from_write(e, "user"))?,
    };
    tx.commit().await?;
    Ok(id)
}

#[derive(Debug, Clone, FromRow)]
pub struct SsoSession {
    pub id: i64,
    pub subject: String,
    pub id_token: Option<String>,
    pub expires_at: String,
    pub last_seen_at: String,
}

pub struct NewSsoSession<'a> {
    pub token_hash: &'a str,
    pub subject: &'a str,
    pub id_token: Option<&'a str>,
    pub expires_at: &'a str,
    pub user_agent: Option<&'a str>,
    pub remote_addr: Option<&'a str>,
}

pub async fn create_sso_session(
    ex: impl SqliteExecutor<'_>,
    s: NewSsoSession<'_>,
) -> DbResult<i64> {
    let ts = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO sso_sessions (token_hash, subject, id_token, created_at, expires_at, last_seen_at, user_agent, remote_addr) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(s.token_hash)
    .bind(s.subject)
    .bind(s.id_token)
    .bind(&ts)
    .bind(s.expires_at)
    .bind(&ts)
    .bind(s.user_agent.map(|ua| ua.chars().take(256).collect::<String>()))
    .bind(s.remote_addr)
    .fetch_one(ex)
    .await?)
}

/// A live (unexpired) single-sign-on session by token hash.
pub async fn find_sso_session(
    ex: impl SqliteExecutor<'_>,
    token_hash: &str,
    now_ts: &str,
) -> DbResult<Option<SsoSession>> {
    Ok(sqlx::query_as(
        "SELECT id, subject, id_token, expires_at, last_seen_at FROM sso_sessions \
         WHERE token_hash = ? AND expires_at > ?",
    )
    .bind(token_hash)
    .bind(now_ts)
    .fetch_optional(ex)
    .await?)
}

pub async fn touch_sso_session(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<()> {
    sqlx::query("UPDATE sso_sessions SET last_seen_at = ? WHERE id = ?")
        .bind(now())
        .bind(id)
        .execute(ex)
        .await?;
    Ok(())
}

/// Ends a single-sign-on session; returns it (for the provider's sign-out) when it existed.
pub async fn delete_sso_session(
    ex: impl SqliteExecutor<'_>,
    token_hash: &str,
) -> DbResult<Option<SsoSession>> {
    Ok(sqlx::query_as(
        "DELETE FROM sso_sessions WHERE token_hash = ? \
         RETURNING id, subject, id_token, expires_at, last_seen_at",
    )
    .bind(token_hash)
    .fetch_optional(ex)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_admin_is_unique_and_sessions_expire() {
        let db = crate::test_support::db().await;
        assert_eq!(user_count(db.pool()).await.unwrap(), 0);
        let id = create_first_admin(&db, " admin ", "$hash").await.unwrap();
        assert!(matches!(
            create_first_admin(&db, "other", "$hash").await,
            Err(DbError::Conflict(_))
        ));
        let user = find_user_by_name(db.pool(), "ADMIN")
            .await
            .unwrap()
            .expect("case-insensitive lookup");
        assert_eq!(user.id, id);
        assert_eq!(user.username, "admin", "username is trimmed");

        create_session(
            db.pool(),
            NewSession {
                token_hash: "live",
                user_id: id,
                expires_at: "2999-01-01T00:00:00Z",
                user_agent: Some("test"),
                remote_addr: None,
            },
        )
        .await
        .unwrap();
        create_session(
            db.pool(),
            NewSession {
                token_hash: "dead",
                user_id: id,
                expires_at: "2000-01-01T00:00:00Z",
                user_agent: None,
                remote_addr: None,
            },
        )
        .await
        .unwrap();
        let ts = now();
        assert!(
            find_session(db.pool(), "live", &ts)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_session(db.pool(), "dead", &ts)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(purge_expired_sessions(db.pool()).await.unwrap(), 1);
        assert_eq!(delete_user_sessions(db.pool(), id, None).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn password_reset_upserts_the_administrator() {
        let db = crate::test_support::db().await;
        assert!(first_admin(db.pool()).await.unwrap().is_none());
        let id = upsert_admin(&db, " boss ", "$one").await.unwrap();
        let again = upsert_admin(&db, "chief", "$two").await.unwrap();
        assert_eq!(id, again, "the one account is renamed, not duplicated");
        let user = first_admin(db.pool()).await.unwrap().unwrap();
        assert_eq!(
            (user.username.as_str(), user.password_hash.as_str()),
            ("chief", "$two")
        );
        assert_eq!(user_count(db.pool()).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn single_sign_on_sessions() {
        let db = crate::test_support::db().await;
        for (hash, expires) in [
            ("live", "2999-01-01T00:00:00Z"),
            ("old", "2000-01-01T00:00:00Z"),
        ] {
            create_sso_session(
                db.pool(),
                NewSsoSession {
                    token_hash: hash,
                    subject: "fox@example.com",
                    id_token: Some("id.token.sig"),
                    expires_at: expires,
                    user_agent: None,
                    remote_addr: Some("127.0.0.1"),
                },
            )
            .await
            .unwrap();
        }
        let ts = now();
        let live = find_sso_session(db.pool(), "live", &ts)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(live.subject, "fox@example.com");
        touch_sso_session(db.pool(), live.id).await.unwrap();
        assert!(
            find_sso_session(db.pool(), "old", &ts)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(purge_expired_sessions(db.pool()).await.unwrap(), 1);
        let ended = delete_sso_session(db.pool(), "live")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ended.id_token.as_deref(), Some("id.token.sig"));
        assert!(
            delete_sso_session(db.pool(), "live")
                .await
                .unwrap()
                .is_none()
        );
    }
}
