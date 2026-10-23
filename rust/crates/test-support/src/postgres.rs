use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use postgresql_embedded::Settings;
use postgresql_embedded::blocking::PostgreSQL;

use crate::{
    TestResult,
    net::{LOCAL_HOST, free_port},
};

const DATABASE: &str = "ente_test";

pub struct Postgres {
    inner: PostgreSQL,
}

impl Postgres {
    pub fn host(&self) -> &str {
        &self.inner.settings().host
    }

    pub fn port(&self) -> u16 {
        self.inner.settings().port
    }

    pub fn username(&self) -> &str {
        &self.inner.settings().username
    }

    pub fn password(&self) -> &str {
        &self.inner.settings().password
    }

    pub fn database(&self) -> &str {
        DATABASE
    }

    pub fn execute_sql(&self, sql: &str) -> TestResult {
        let output = Command::new(
            self.inner
                .settings()
                .binary_dir()
                .join(format!("psql{}", std::env::consts::EXE_SUFFIX)),
        )
        .env("PGPASSWORD", self.password())
        .args([
            "-X",
            "-v",
            "ON_ERROR_STOP=1",
            "-h",
            self.host(),
            "-p",
            &self.port().to_string(),
            "-U",
            self.username(),
            "-d",
            self.database(),
            "-c",
            sql,
        ])
        .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(())
    }
}

pub fn start() -> TestResult<Postgres> {
    let settings = Settings {
        temporary: true,
        host: LOCAL_HOST.to_string(),
        port: free_port()?,
        installation_dir: install_dir()?,
        ..Settings::default()
    };
    let mut inner = PostgreSQL::new(settings);
    inner.setup()?;
    restore_utc_zone(&inner.settings().binary_dir())?;
    inner.start()?;
    inner.create_database(DATABASE)?;
    Ok(Postgres { inner })
}

// fork: postgresql_archive's tar extractor skips hard links, and the theseus
// archives store the tz database's aliases (`UTC` among them) as hard links.
// Without `UTC` the server rejects the `TimeZone=UTC` that sqlx sends on
// connect, failing every museum-backed test. Copy it back from an alias that
// survived extraction.
fn restore_utc_zone(binary_dir: &Path) -> TestResult {
    const UTC_ALIASES: [&str; 7] = [
        "Etc/UTC",
        "Etc/UCT",
        "Etc/Universal",
        "Etc/Zulu",
        "UCT",
        "Universal",
        "Zulu",
    ];
    let Some(prefix) = binary_dir.parent() else {
        return Ok(());
    };
    for tz_dir in [
        prefix.join("share").join("timezone"),
        prefix.join("share").join("postgresql").join("timezone"),
    ] {
        let utc = tz_dir.join("UTC");
        if !tz_dir.is_dir() || utc.is_file() {
            continue;
        }
        if let Some(source) = UTC_ALIASES
            .iter()
            .map(|alias| tz_dir.join(alias))
            .find(|path| path.is_file())
        {
            fs::copy(source, utc)?;
        }
    }
    Ok(())
}

// postgresql_embedded defaults its binary to a `~/.theseus` dir in $HOME; keep
// that name but under the conventional cache directory instead.
fn install_dir() -> TestResult<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or("could not resolve a cache directory")?
        .join(".theseus")
        .join("postgresql"))
}
