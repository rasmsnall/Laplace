use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use chrono::Utc;
use futures::TryStreamExt;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreScheme};
use url::Url;

use super::Report;
use super::files::{self, Observation, RemoteFile};
use crate::config::Storage;

pub async fn check(
    config: &Storage,
    previous: &HashMap<String, Observation>,
    baseline: bool,
) -> Result<Report> {
    let (store, prefix) = open(&config.url)?;
    let objects: Vec<_> = store.list(Some(&prefix)).try_collect().await?;

    let remote_files = objects
        .into_iter()
        .filter_map(|object| {
            Some(RemoteFile {
                key: object.location.to_string(),
                name: object.location.filename()?.to_owned(),
                size: object.size,
                modified: object.last_modified,
            })
        })
        .collect();

    files::evaluate(&config.files, remote_files, previous, baseline, Utc::now())
}

/// credentials come from the environment, the same variables the azure and gcp sdks use
pub(super) fn open(location: &str) -> Result<(Arc<dyn ObjectStore>, Path)> {
    // `c:\data` would parse as a url with scheme `c`, so only `://` makes a url
    let url = if location.contains("://") {
        Url::parse(location)?
    } else {
        Url::from_directory_path(std::path::absolute(location)?)
            .map_err(|()| anyhow::anyhow!("not a valid path: {location}"))?
    };
    let (scheme, prefix) = ObjectStoreScheme::parse(&url)?;

    let store: Arc<dyn ObjectStore> = match scheme {
        ObjectStoreScheme::GoogleCloudStorage => Arc::new(
            GoogleCloudStorageBuilder::from_env()
                .with_url(url.as_str())
                .build()?,
        ),
        ObjectStoreScheme::MicrosoftAzure => Arc::new(
            MicrosoftAzureBuilder::from_env()
                .with_url(url.as_str())
                .build()?,
        ),
        ObjectStoreScheme::Local => Arc::new(LocalFileSystem::new()),
        other => bail!("unsupported storage: {other:?}"),
    };
    Ok((store, prefix))
}
