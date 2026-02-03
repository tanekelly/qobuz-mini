use crate::{AudioQuality, Error, Result, Tracklist};
use serde_json::to_string;
use sqlx::types::Json;
use sqlx::{Pool, Row, Sqlite, SqlitePool, sqlite::SqliteConnectOptions};
use std::path::{Path, PathBuf};

pub struct Database {
    pool: Pool<Sqlite>,
    database_path: PathBuf,
}

impl Database {
    pub async fn new() -> Result<Self> {
        let database_url = if let Ok(url) = std::env::var("DATABASE_URL") {
            PathBuf::from(url.replace("sqlite://", ""))
        } else {
            let Some(mut url) = dirs::data_local_dir() else {
                return Err(Error::DatabaseLocationError);
            };
            url.push("qobuz-player");

            if !url.exists() {
                let Ok(_) = std::fs::create_dir_all(&url) else {
                    return Err(Error::DatabaseLocationError);
                };
            }

            url.push("data.db");

            url
        };

        let options = SqliteConnectOptions::new()
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .filename(&database_url)
            .create_if_missing(true);

        let pool = SqlitePool::connect_with(options).await?;

        Database::init(pool, database_url).await
    }

    async fn init(pool: sqlx::Pool<sqlx::Sqlite>, database_path: PathBuf) -> Result<Self> {
        if let Err(e) = sqlx::migrate!("./migrations").run(&pool).await {
            let error_msg = format!("{}", e);
            if error_msg.contains("was previously applied but is missing") ||
                error_msg.contains("missing in the resolved migrations") {
                pool.close().await;
                delete_database_files(&database_path)?;
                return Self::create_fresh_database(database_path).await;
            }
            return Err(e.into());
        }

        create_credentials_row(&pool).await?;
        create_configuration(&pool).await?;

        Ok(Self { pool, database_path })
    }

    async fn create_fresh_database(database_path: PathBuf) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .filename(&database_path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        create_credentials_row(&pool).await?;
        create_configuration(&pool).await?;
        Ok(Self { pool, database_path })
    }

    pub async fn set_username(&self, username: String) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE credentials
            SET username=?1
            WHERE ROWID = 1
            "#,
            username
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_password(&self, password: String) -> Result<()> {
        let md5_pw = format!("{:x}", md5::compute(password));
        sqlx::query!(
            r#"
            UPDATE credentials
            SET password=?1
            WHERE ROWID = 1
            "#,
            md5_pw
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn clear_credentials(&self) -> Result<()> {
        sqlx::query("UPDATE credentials SET username=NULL, password=NULL WHERE ROWID = 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_tracklist(&self, tracklist: &Tracklist) -> Result<()> {
        let serialized = to_string(&tracklist)?;

        sqlx::query!(
            r#"
           delete from tracklist
        "#
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO tracklist (tracklist) VALUES (?1);
        "#,
        )
        .bind(&serialized)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_tracklist(&self) -> Option<Tracklist> {
        let row = sqlx::query_as!(
            TracklistDb,
            r#"
            SELECT tracklist as "tracklist: Json<Tracklist>" FROM tracklist
        "#
        )
        .fetch_one(&self.pool)
        .await;

        row.ok().map(|x| x.tracklist.0)
    }

    pub async fn set_volume(&self, volume: f32) -> Result<()> {
        sqlx::query!(
            r#"
           delete from volume
        "#
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO volume (volume) VALUES (?1);
        "#,
        )
        .bind(volume)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_volume(&self) -> Option<f32> {
        let row = sqlx::query_as!(
            VolumeDb,
            r#"
            SELECT volume FROM volume
        "#
        )
        .fetch_one(&self.pool)
        .await;

        row.ok().map(|x| x.volume as f32)
    }

    pub async fn set_max_audio_quality(&self, quality: AudioQuality) -> Result<()> {
        let quality_id = quality as i32;

        sqlx::query!(
            r#"
            UPDATE configuration
            SET max_audio_quality=?1
            WHERE ROWID = 1
            "#,
            quality_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_credentials(&self) -> Result<DatabaseCredentials> {
        Ok(sqlx::query_as!(
            DatabaseCredentials,
            r#"
            SELECT * FROM credentials
            WHERE ROWID = 1;
            "#
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_configuration(&self) -> Result<DatabaseConfiguration> {
        let row = sqlx::query(
            r#"
            SELECT max_audio_quality, audio_device_name, preferred_genre_id, time_stretch_ratio, pitch_semitones, pitch_cents FROM configuration
            WHERE ROWID = 1;
            "#
        )
        .fetch_one(&self.pool)
        .await?;

        let time_stretch_ratio: f32 = row
            .get::<Option<f64>, _>("time_stretch_ratio")
            .unwrap_or(1.0) as f32;
        let pitch_semitones: i16 = row
            .get::<Option<i32>, _>("pitch_semitones")
            .unwrap_or(0) as i16;
        let pitch_cents: i16 = row
            .get::<Option<i32>, _>("pitch_cents")
            .unwrap_or(0) as i16;
        Ok(DatabaseConfiguration {
            max_audio_quality: row.get("max_audio_quality"),
            audio_device_name: row.get("audio_device_name"),
            preferred_genre_id: row.get::<Option<i64>, _>("preferred_genre_id"),
            time_stretch_ratio,
            pitch_semitones,
            pitch_cents,
        })
    }

    pub async fn set_audio_device(&self, device_name: Option<String>) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE configuration
            SET audio_device_name=?1
            WHERE ROWID = 1
            "#,
        )
        .bind(device_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_preferred_genre_id(&self, genre_id: Option<i64>) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE configuration
            SET preferred_genre_id=?1
            WHERE ROWID = 1
            "#,
        )
        .bind(genre_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_time_stretch_ratio(&self, ratio: f32) -> Result<()> {
        let ratio = ratio.clamp(0.5, 2.0) as f64;
        sqlx::query(
            r#"
            UPDATE configuration
            SET time_stretch_ratio=?1
            WHERE ROWID = 1
            "#,
        )
        .bind(ratio)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_pitch_semitones(&self, semitones: i16) -> Result<()> {
        let semitones = semitones.clamp(-12, 12) as i32;
        sqlx::query(
            r#"
            UPDATE configuration
            SET pitch_semitones=?1
            WHERE ROWID = 1
            "#,
        )
        .bind(semitones)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_pitch_cents(&self, cents: i16) -> Result<()> {
        let cents = cents.clamp(-100, 100) as i32;
        sqlx::query(
            r#"
            UPDATE configuration
            SET pitch_cents=?1
            WHERE ROWID = 1
            "#,
        )
        .bind(cents)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn add_rfid_reference(
        &self,
        rfid_id: String,
        reference: ReferenceType,
    ) -> Result<()> {
        match reference {
            ReferenceType::Album(id) => {
                let id = Some(id);

                sqlx::query!(
                    "INSERT INTO rfid_references (id, reference_type, album_id, playlist_id) VALUES ($1, $2, $3, $4) ON CONFLICT(id) DO UPDATE SET reference_type = excluded.reference_type, album_id = excluded.album_id, playlist_id = excluded.playlist_id RETURNING *;",
                    rfid_id,
                    1,
                    id,
                    None::<u32>,
                ).fetch_one(&self.pool).await?;
            }
            ReferenceType::Playlist(id) => {
                let id = Some(id);

                sqlx::query!(
                    "INSERT INTO rfid_references (id, reference_type, album_id, playlist_id) VALUES ($1, $2, $3, $4) ON CONFLICT(id) DO UPDATE SET reference_type = excluded.reference_type, album_id = excluded.album_id, playlist_id = excluded.playlist_id RETURNING *;",
                    rfid_id,
                    2,
                    None::<String>,
                    id,
                ).fetch_one(&self.pool).await?;
            }
        }
        Ok(())
    }

    pub async fn get_reference(&self, id: &str) -> Option<LinkRequest> {
        let db_reference = match sqlx::query_as!(
            RFIDReference,
            "SELECT * FROM rfid_references WHERE ID = $1;",
            id
        )
        .fetch_one(&self.pool)
        .await
        {
            Ok(res) => res,
            Err(_) => return None,
        };

        match db_reference.reference_type {
            ReferenceTypeDatabase::Album => Some(LinkRequest::Album(db_reference.album_id?)),
            ReferenceTypeDatabase::Playlist => {
                Some(LinkRequest::Playlist(db_reference.playlist_id? as u32))
            }
        }
    }

    pub async fn clean_up_cache_entries(&self, older_than: time::Duration) -> Result<Vec<PathBuf>> {
        let cutoff = time::OffsetDateTime::now_utc() - older_than;
        let cutoff_str = cutoff
            .format(&time::format_description::well_known::Rfc3339)
            .expect("infallible");

        let rows = sqlx::query!(
            "SELECT path FROM cache_entries WHERE last_opened < ?",
            cutoff_str
        )
        .fetch_all(&self.pool)
        .await?;

        sqlx::query!(
            "DELETE FROM cache_entries WHERE last_opened < ?",
            cutoff_str
        )
        .execute(&self.pool)
        .await?;

        let paths: Vec<PathBuf> = rows
            .into_iter()
            .map(|row| PathBuf::from(row.path))
            .collect();

        Ok(paths)
    }

    pub async fn set_cache_entry(&self, path: &Path) {
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .expect("infallible");

        let path_str: String = path.to_string_lossy().into_owned();

        sqlx::query!(
            r#"
                INSERT INTO cache_entries (path, last_opened)
                VALUES (?, ?)
                ON CONFLICT(path) DO UPDATE SET
                    path = excluded.path,
                    last_opened = excluded.last_opened
            "#,
            path_str,
            now
        )
        .execute(&self.pool)
        .await
        .expect("infallible");
    }

    pub async fn refresh_database(&self) -> Result<()> {
        self.pool.close().await;
        delete_database_files(&self.database_path)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum LinkRequest {
    Album(String),
    Playlist(u32),
}

pub enum ReferenceType {
    Album(String),
    Playlist(u32),
}

#[derive(sqlx::FromRow)]
struct RFIDReference {
    #[allow(dead_code)]
    id: String,
    reference_type: ReferenceTypeDatabase,
    album_id: Option<String>,
    playlist_id: Option<i64>,
}

enum ReferenceTypeDatabase {
    Album = 1,
    Playlist = 2,
}

impl From<i64> for ReferenceTypeDatabase {
    fn from(value: i64) -> Self {
        match value {
            1 => ReferenceTypeDatabase::Album,
            2 => ReferenceTypeDatabase::Playlist,
            _ => panic!("Unable to parse reference type!"),
        }
    }
}

pub struct DatabaseCredentials {
    pub username: Option<String>,
    pub password: Option<String>,
}

pub struct DatabaseConfiguration {
    pub max_audio_quality: i64,
    pub audio_device_name: Option<String>,
    pub preferred_genre_id: Option<i64>,
    pub time_stretch_ratio: f32,
    pub pitch_semitones: i16,
    pub pitch_cents: i16,
}

#[derive(Debug, sqlx::FromRow, serde::Deserialize)]
struct TracklistDb {
    tracklist: Json<Tracklist>,
}

#[derive(Debug, sqlx::FromRow, serde::Deserialize)]
struct VolumeDb {
    volume: f64,
}

fn delete_database_files(db_path: &PathBuf) -> Result<()> {
    if db_path.exists() {
        std::fs::remove_file(db_path)?;
    }
    let mut wal_path = db_path.clone();
    wal_path.set_extension("db-wal");
    if wal_path.exists() {
        std::fs::remove_file(&wal_path)?;
    }
    let mut shm_path = db_path.clone();
    shm_path.set_extension("db-shm");
    if shm_path.exists() {
        std::fs::remove_file(&shm_path)?;
    }
    Ok(())
}

async fn create_credentials_row(pool: &Pool<Sqlite>) -> Result<()> {
    let rowid = 1;

    sqlx::query!(
        r#"
            INSERT OR IGNORE INTO credentials (ROWID) VALUES (?1);
            "#,
        rowid
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn create_configuration(pool: &Pool<Sqlite>) -> Result<()> {
    let rowid = 1;
    sqlx::query!(
        r#"
            INSERT OR IGNORE INTO configuration (ROWID) VALUES (?1);
            "#,
        rowid
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Duration, OffsetDateTime};

    #[sqlx::test]
    async fn clean_up_cache_entries(pool: sqlx::Pool<sqlx::Sqlite>) {
        let dummy_path = PathBuf::from(":memory:");
        let db = Database::init(pool, dummy_path).await.unwrap();

        let old_path_str = "path/old";
        let old_path = Path::new(old_path_str);
        let new_path_str = "path/new";
        let new_path = Path::new(new_path_str);
        db.set_cache_entry(old_path).await;
        db.set_cache_entry(new_path).await;

        let old_time = OffsetDateTime::now_utc() - Duration::days(10);
        let old_time = old_time
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();

        sqlx::query!(
            "UPDATE cache_entries SET last_opened = ? WHERE path = ?",
            old_time,
            old_path_str
        )
        .execute(&db.pool)
        .await
        .unwrap();

        let deleted = db.clean_up_cache_entries(Duration::days(5)).await.unwrap();

        let remaining: Vec<_> = sqlx::query!("SELECT path FROM cache_entries")
            .fetch_all(&db.pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.path)
            .collect();

        assert_eq!(remaining, vec![new_path_str]);
        assert_eq!(deleted, vec![old_path]);
    }
}
