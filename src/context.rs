use anyhow::Result;

use crate::api::Client;
use crate::credentials::{self, Credentials};

/// State shared by every command: the API client, the login it uses, and the global flags.
pub struct Ctx {
    pub client: Client,
    pub creds: Credentials,
    pub json: bool,
    pub yes: bool,
}

impl Ctx {
    pub fn new(site: Option<String>, json: bool, yes: bool) -> Result<Self> {
        let creds = credentials::resolve(site.as_deref())?;
        let client = Client::new(&creds)?;
        Ok(Self {
            client,
            creds,
            json,
            yes,
        })
    }

    /// Web link to an issue on the login's site.
    pub fn browse_url(&self, key: &str) -> String {
        self.creds.browse_url(key)
    }
}
