use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use chrono::{DateTime, Utc};

use super::files::Observation;
use ssh2::{HashType, Session};

use super::Report;
use super::files::{self, RemoteFile};
use crate::config::Sftp;

const TIMEOUT: Duration = Duration::from_secs(15);

/// `trusted_key` is the key seen on earlier polls; a pinned `host_key` in the config wins over it
pub fn check(
    config: &Sftp,
    trusted_key: Option<&str>,
    previous: &HashMap<String, Observation>,
    baseline: bool,
) -> Result<Report> {
    let session = handshake(config)?;
    let seen = fingerprint(&session)?;

    if let Some(expected) = config.host_key.as_deref().or(trusted_key)
        && seen != expected
    {
        // a changed key can mean someone is in the middle; never send credentials to it
        return Ok(Report {
            problem: Some(format!(
                "host key changed from {expected} to {seen}, not logging in until it is confirmed"
            )),
            host_key: Some(seen),
            ..Report::default()
        });
    }

    authenticate(&session, config)?;
    let listing = session.sftp()?.readdir(Path::new(&config.dir))?;
    let remote_files = listing
        .into_iter()
        .filter(|(_, stat)| stat.is_file())
        .filter_map(|(path, stat)| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some(RemoteFile {
                key: name.clone(),
                name,
                size: stat.size.unwrap_or(0),
                modified: DateTime::from_timestamp(stat.mtime? as i64, 0)?,
            })
        })
        .collect();

    Ok(Report {
        host_key: Some(seen),
        ..files::evaluate(&config.files, remote_files, previous, baseline, Utc::now())?
    })
}

fn handshake(config: &Sftp) -> Result<Session> {
    let address = (config.host.as_str(), config.port)
        .to_socket_addrs()?
        .next()
        .with_context(|| format!("{} did not resolve", config.host))?;

    let mut session = Session::new()?;
    session.set_tcp_stream(TcpStream::connect_timeout(&address, TIMEOUT)?);
    session.set_timeout(TIMEOUT.as_millis() as u32);
    session.handshake()?;
    Ok(session)
}

fn fingerprint(session: &Session) -> Result<String> {
    let hash = session
        .host_key_hash(HashType::Sha256)
        .context("server sent no host key")?;
    Ok(format!("SHA256:{}", STANDARD_NO_PAD.encode(hash)))
}

fn authenticate(session: &Session, config: &Sftp) -> Result<()> {
    match (&config.key_file, &config.password_env) {
        (Some(key_file), _) => session.userauth_pubkey_file(&config.user, None, key_file, None)?,
        (None, Some(variable)) => {
            let password =
                std::env::var(variable).with_context(|| format!("{variable} is not set"))?;
            session.userauth_password(&config.user, &password)?
        }
        (None, None) => bail!("no credentials configured"),
    }
    Ok(())
}
