//! settings changed in the dashboard. environment variables give the defaults, the database
//! holds what an admin changed, and every replica rereads them on each scheduler tick.

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde::Serialize;
use sqlx::PgPool;

#[derive(Clone, Serialize)]
pub struct Settings {
    /// zone for schedules without their own and for report months
    #[serde(serialize_with = "zone_name")]
    pub timezone: chrono_tz::Tz,
    /// days of events and calls kept in postgres
    pub retention_days: i64,
    /// jobs reporting under a new name create their flow
    pub auto_register: bool,
}

fn zone_name<S: serde::Serializer>(zone: &chrono_tz::Tz, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(zone.name())
}

pub const MAX_RETENTION_DAYS: i64 = 3650;

impl Settings {
    /// applies one change, checking it the same way whether it came from the database or an admin
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "timezone" => {
                self.timezone = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("timezone {value:?} is not known"))?;
            }
            "retention_days" => match value.parse() {
                Ok(days) if (1..=MAX_RETENTION_DAYS).contains(&days) => self.retention_days = days,
                _ => bail!("retention_days: a whole number of days from 1 to {MAX_RETENTION_DAYS}"),
            },
            "auto_register" => match value {
                "true" | "false" => self.auto_register = value == "true",
                _ => bail!("auto_register: true or false"),
            },
            other => bail!("no setting called {other}"),
        }
        Ok(())
    }

    pub async fn load(pool: &PgPool, defaults: &Settings) -> Result<Settings> {
        let stored: Vec<(String, String)> = sqlx::query_as("select key, value from settings")
            .fetch_all(pool)
            .await?;
        let mut settings = defaults.clone();
        for (key, value) in stored {
            // a value that no longer parses falls back to the default rather than stopping laplace
            if let Err(error) = settings.set(&key, &value) {
                eprintln!("ignoring stored setting: {error:#}");
            }
        }
        Ok(settings)
    }

    /// checks every change first, so a bad value leaves nothing half saved
    pub async fn save(
        pool: &PgPool,
        current: &Settings,
        changes: &HashMap<String, String>,
        by: &str,
    ) -> Result<Settings> {
        let mut next = current.clone();
        for (key, value) in changes {
            next.set(key, value)?;
        }
        let mut tx = pool.begin().await?;
        for (key, value) in changes {
            sqlx::query(
                "insert into settings (key, value, changed_by) values ($1, $2, $3)
                 on conflict (key) do update set value = excluded.value, changed_by = excluded.changed_by, changed_at = now()",
            )
            .bind(key)
            .bind(value)
            .bind(by)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Settings {
        Settings {
            timezone: chrono_tz::Europe::Stockholm,
            retention_days: 30,
            auto_register: true,
        }
    }

    #[test]
    fn accepts_good_values_and_explains_bad_ones() {
        let mut settings = defaults();
        settings.set("timezone", "Europe/Helsinki").unwrap();
        settings.set("retention_days", "90").unwrap();
        settings.set("auto_register", "false").unwrap();
        assert_eq!(settings.timezone, chrono_tz::Europe::Helsinki);
        assert_eq!(
            (settings.retention_days, settings.auto_register),
            (90, false)
        );

        assert!(settings.set("timezone", "Mars/Olympus").is_err());
        assert!(settings.set("retention_days", "0").is_err());
        assert!(settings.set("auto_register", "yes").is_err());
        assert!(settings.set("colour", "gold").is_err());
    }
}
