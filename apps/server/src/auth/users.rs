//! User repository (infrastructure; ADR-001 boundary: SQL lives here).

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// A persisted internal User. `password_hash` is the Argon2id PHC string;
/// it never leaves this layer through API responses.
#[derive(Debug, sqlx::FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub username: String,
    pub display_name: String,
    pub status: String,
    pub password_hash: String,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug)]
pub enum CreateUserError {
    DuplicateUsername,
    Database(sqlx::Error),
}

/// Create a user with a username and a pre-hashed password. The username
/// is normalized defensively here (callers must already normalize), and
/// the database's lower(username) unique index guarantees uniqueness
/// under the normalization rule regardless of the caller.
/// Duplicate normalized usernames surface as `DuplicateUsername`.
pub async fn create_user(
    pool: &PgPool,
    username: &str,
    display_name: &str,
    password_hash: &str,
) -> Result<UserRow, CreateUserError> {
    let normalized = crate::auth::identity::normalize_username(username);
    let row = sqlx::query_as::<_, UserRow>(
        "INSERT INTO users (username, display_name, password_hash) \
         VALUES ($1, $2, $3) \
         RETURNING id, username, display_name, status, password_hash, \
                   created_at, updated_at, last_login_at",
    )
    .bind(&normalized)
    .bind(display_name)
    .bind(password_hash)
    .fetch_one(pool)
    .await;
    match row {
        Ok(user) => Ok(user),
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            Err(CreateUserError::DuplicateUsername)
        }
        Err(error) => Err(CreateUserError::Database(error)),
    }
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, status, password_hash, \
                created_at, updated_at, last_login_at \
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn find_by_normalized_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as::<_, UserRow>(
        "SELECT id, username, display_name, status, password_hash, \
                created_at, updated_at, last_login_at \
         FROM users WHERE username = $1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await
}

/// Record a successful login timestamp (login metadata, STEP-002 §16).
pub async fn touch_last_login(
    pool: &PgPool,
    user_id: Uuid,
    at: OffsetDateTime,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET last_login_at = $2, updated_at = $2 WHERE id = $1")
        .bind(user_id)
        .bind(at)
        .execute(pool)
        .await
        .map(|_| ())
}
