#[cfg(test)]
use std::{collections::HashMap, sync::Mutex};

use anyhow::Result;
#[cfg(test)]
use anyhow::anyhow;

pub trait SecretStore: Send + Sync {
    fn set(&self, profile_id: &str, secret: &str) -> Result<()>;
    fn get(&self, profile_id: &str) -> Result<Option<String>>;
    fn delete(&self, profile_id: &str) -> Result<()>;
}

pub struct WindowsSecretStore;

impl WindowsSecretStore {
    fn entry(profile_id: &str) -> Result<keyring::Entry> {
        let service = format!("anydoc-assistant/{profile_id}");
        keyring::Entry::new(&service, "api-key").map_err(Into::into)
    }
}

impl SecretStore for WindowsSecretStore {
    fn set(&self, profile_id: &str, secret: &str) -> Result<()> {
        Self::entry(profile_id)?.set_password(secret)?;
        Ok(())
    }

    fn get(&self, profile_id: &str) -> Result<Option<String>> {
        match Self::entry(profile_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn delete(&self, profile_id: &str) -> Result<()> {
        match Self::entry(profile_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemorySecretStore {
    secrets: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl SecretStore for MemorySecretStore {
    fn set(&self, profile_id: &str, secret: &str) -> Result<()> {
        self.secrets
            .lock()
            .map_err(|_| anyhow!("secret store lock poisoned"))?
            .insert(profile_id.into(), secret.into());
        Ok(())
    }

    fn get(&self, profile_id: &str) -> Result<Option<String>> {
        Ok(self
            .secrets
            .lock()
            .map_err(|_| anyhow!("secret store lock poisoned"))?
            .get(profile_id)
            .cloned())
    }

    fn delete(&self, profile_id: &str) -> Result<()> {
        self.secrets.lock().map_err(|_| anyhow!("secret store lock poisoned"))?.remove(profile_id);
        Ok(())
    }
}
