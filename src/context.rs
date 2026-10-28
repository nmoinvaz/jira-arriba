use anyhow::Result;

use crate::api::Client;
use crate::credentials::{self, Credentials};

/// State shared by every command: the API client, the login it uses, and the global flags.
pub struct Ctx {
    pub client: Client,
    pub creds: Credentials,
    /// The --site flag, kept so the login can be read again.
    pub site: Option<String>,
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
            site,
            json,
            yes,
        })
    }

    /// Read the login again, as when its token is about to expire, and use it from now on.
    pub fn refresh_login(&mut self) -> Result<()> {
        let creds = credentials::resolve(self.site.as_deref())?;
        self.client = Client::new(&creds)?;
        self.creds = creds;
        Ok(())
    }

    /// Web link to an issue on the login's site.
    pub fn browse_url(&self, key: &str) -> String {
        self.creds.browse_url(key)
    }
}
