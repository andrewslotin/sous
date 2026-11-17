//! The household profile: five fixed plain-text sections with a revision each.
//!
//! [`update_section`] is the only code that writes `profile_sections`; MCP and HTTP both call it.
use crate::model::{Profile, ProfileSection, SectionKey};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::fmt::Write;

/// The longest section text, in Unicode scalar values, counted as submitted.
pub const MAX_TEXT_CHARS: usize = 4000;

/// Why a section update failed.
#[derive(Debug)]
pub enum UpdateError {
    /// Safe message naming the problem (text too long).
    InvalidInput(String),
    /// `expected_revision` is not the stored revision; `current` is the section as now saved.
    Conflict { current: ProfileSection },
    /// Logged by the caller; never shown.
    Database(sqlx::Error),
}

impl From<sqlx::Error> for UpdateError {
    fn from(error: sqlx::Error) -> Self {
        UpdateError::Database(error)
    }
}

/// Lowercase hex SHA-256 of the key, a zero byte, then the stored text.
fn revision(key: SectionKey, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            let _ = write!(hex, "{b:02x}");
            hex
        })
}

fn section(key: SectionKey, text: String) -> ProfileSection {
    ProfileSection {
        key,
        title: key.title().to_owned(),
        revision: revision(key, &text),
        text,
    }
}

/// All five sections in one read snapshot.
pub async fn load_profile(pool: &SqlitePool) -> Result<Profile, sqlx::Error> {
    // One statement is one snapshot.
    let rows = sqlx::query("SELECT key, text FROM profile_sections")
        .fetch_all(pool)
        .await?;
    let mut texts: HashMap<String, String> = HashMap::new();
    for row in &rows {
        texts.insert(row.try_get("key")?, row.try_get("text")?);
    }
    let sections = SectionKey::ALL
        .into_iter()
        .map(|key| {
            texts
                .remove(key.as_str())
                .map(|text| section(key, text))
                .ok_or(sqlx::Error::RowNotFound)
        })
        .collect::<Result<_, _>>()?;
    Ok(Profile { sections })
}

/// Replace one section's text. The text must be at most [`MAX_TEXT_CHARS`] characters as submitted; then
/// CRLF becomes LF and nothing else changes. In one `BEGIN IMMEDIATE` transaction the stored revision is
/// compared with `expected_revision` (mismatch: `Conflict` with the stored section), then the text is
/// updated. Saving the same text again with the right revision succeeds with the same revision.
pub async fn update_section(
    write_pool: &SqlitePool,
    key: SectionKey,
    text: &str,
    expected_revision: &str,
) -> Result<ProfileSection, UpdateError> {
    update_section_with_pause(
        write_pool,
        key,
        text,
        expected_revision,
        std::future::ready(()),
    )
    .await
}

/// Test seam: `pause` is awaited inside the write transaction after the revision check, before the update.
#[doc(hidden)]
pub async fn update_section_with_pause(
    write_pool: &SqlitePool,
    key: SectionKey,
    text: &str,
    expected_revision: &str,
    pause: impl std::future::Future<Output = ()>,
) -> Result<ProfileSection, UpdateError> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(UpdateError::InvalidInput(format!(
            "Text must be at most {MAX_TEXT_CHARS} characters."
        )));
    }
    let text = text.replace("\r\n", "\n");

    // BEGIN IMMEDIATE takes the write lock before reading, so the revision check is atomic with the write.
    let mut tx = write_pool.begin_with("BEGIN IMMEDIATE").await?;
    let outcome = replace(&mut tx, key, text, expected_revision, pause).await;
    match outcome {
        Ok(saved) => {
            tx.commit().await?;
            Ok(saved)
        }
        Err(error) => {
            // Roll back now so the write lock is released before the caller sees the error.
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn replace(
    conn: &mut sqlx::SqliteConnection,
    key: SectionKey,
    text: String,
    expected_revision: &str,
    pause: impl std::future::Future<Output = ()>,
) -> Result<ProfileSection, UpdateError> {
    let stored: String = sqlx::query_scalar("SELECT text FROM profile_sections WHERE key = ?")
        .bind(key.as_str())
        .fetch_one(&mut *conn)
        .await?;
    if revision(key, &stored) != expected_revision {
        return Err(UpdateError::Conflict {
            current: section(key, stored),
        });
    }
    pause.await;
    sqlx::query("UPDATE profile_sections SET text = ? WHERE key = ?")
        .bind(&text)
        .bind(key.as_str())
        .execute(&mut *conn)
        .await?;
    Ok(section(key, text))
}
