//! tokens jobs send on /ping and /calls. only a sha-256 hash is stored; the token itself is
//! shown once, when it is created.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// a busy gateway checks its token on every call; a revoked token stops working within this
const CACHE_FOR: Duration = Duration::from_secs(30);

#[derive(Serialize, sqlx::FromRow)]
pub struct Token {
    pub id: i64,
    pub name: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// returns the token, which cannot be read back later
pub async fn create(pool: &PgPool, name: &str, by: &str) -> Result<String> {
    let mut secret = [0u8; 32];
    rand::rng().fill_bytes(&mut secret);
    let token = format!("lpl_{}", URL_SAFE_NO_PAD.encode(secret));
    sqlx::query("insert into job_tokens (name, hash, created_by) values ($1, $2, $3)")
        .bind(name)
        .bind(hash(&token))
        .bind(by)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn list(pool: &PgPool) -> Result<Vec<Token>> {
    Ok(sqlx::query_as(
        "select id, name, created_by, created_at, last_used_at, revoked_at from job_tokens order by revoked_at nulls first, created_at desc",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn revoke(pool: &PgPool, id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("update job_tokens set revoked_at = now() where id = $1 and revoked_at is null returning name")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// what the database said about a token, remembered for a short while
#[derive(Default)]
pub struct Cache {
    answers: Mutex<HashMap<String, (bool, Instant)>>,
    any_made: Mutex<Option<(bool, Instant)>>,
}

impl Cache {
    /// reporting is open until the first token is made, and stays closed after that, even when every
    /// token is revoked: revoking a leaked token must never open reporting to everyone
    pub async fn required(&self, pool: &PgPool) -> Result<bool> {
        if let Some((any, at)) = *self.any_made.lock().unwrap()
            && at.elapsed() < CACHE_FOR
        {
            return Ok(any);
        }
        let any: bool = sqlx::query_scalar("select exists (select 1 from job_tokens)")
            .fetch_one(pool)
            .await?;
        *self.any_made.lock().unwrap() = Some((any, Instant::now()));
        Ok(any)
    }

    pub async fn valid(&self, pool: &PgPool, token: &str) -> Result<bool> {
        let hashed = hash(token);
        if let Some(&(valid, at)) = self.answers.lock().unwrap().get(&hashed)
            && at.elapsed() < CACHE_FOR
        {
            return Ok(valid);
        }
        let id: Option<i64> =
            sqlx::query_scalar("select id from job_tokens where hash = $1 and revoked_at is null")
                .bind(&hashed)
                .fetch_optional(pool)
                .await?;
        if let Some(id) = id {
            sqlx::query("update job_tokens set last_used_at = now() where id = $1")
                .bind(id)
                .execute(pool)
                .await?;
        }
        // only valid tokens are remembered: there are few of them, while wrong guesses are
        // unlimited and would pile up in memory
        if id.is_some() {
            self.answers
                .lock()
                .unwrap()
                .insert(hashed, (true, Instant::now()));
        }
        Ok(id.is_some())
    }

    /// makes a revocation or a new token count right away on this replica
    pub fn forget(&self) {
        self.answers.lock().unwrap().clear();
        *self.any_made.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::hash;

    #[test]
    fn hashes_are_sha256_hex() {
        assert_eq!(
            hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
