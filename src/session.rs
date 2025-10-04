use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use parking_lot::RwLock;

use crate::auth::{AuthorizationRequest, Flow as AuthFlow, Session as AuthSession};
use crate::reddit::TokenProvider;
use crate::storage::{self, Account};

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("account not found")]
    AccountNotFound,
    #[error("token not found")]
    TokenNotFound,
}

pub struct Manager {
    store: Arc<storage::Store>,
    flow: Arc<AuthFlow>,
    sessions: RwLock<HashMap<i64, AuthSession>>,
    active_id: RwLock<Option<i64>>,
}

impl Manager {
    pub fn new(store: Arc<storage::Store>, flow: Arc<AuthFlow>) -> Result<Self> {
        Ok(Self {
            store,
            flow,
            sessions: RwLock::new(HashMap::new()),
            active_id: RwLock::new(None),
        })
    }

    pub fn close(&self) {
        self.flow.close();
    }

    pub fn load_existing(&self) -> Result<()> {
        let preferred_active = self.store.last_active_account_id()?;
        let accounts = self.store.list_accounts()?;
        let mut prepared_sessions = Vec::new();
        let mut new_active = None;

        for account in accounts {
            if let Some(token) = self.store.get_token(account.id)? {
                let session = self.flow.resume(account.clone(), token)?;
                if preferred_active == Some(account.id) || new_active.is_none() {
                    new_active = Some(account.id);
                }
                prepared_sessions.push((account.id, session));
            }
        }

        {
            let mut sessions = self.sessions.write();
            for (id, session) in prepared_sessions {
                sessions.insert(id, session);
            }
        }

        *self.active_id.write() = new_active;
        if preferred_active != new_active {
            self.store.set_last_active_account_id(new_active)?;
        }
        Ok(())
    }

    pub fn active(&self) -> Option<AuthSession> {
        let sessions = self.sessions.read();
        let active = self.active_id.read();
        active.and_then(|id| sessions.get(&id).cloned())
    }

    pub fn active_account_id(&self) -> Option<i64> {
        *self.active_id.read()
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        self.store.list_accounts()
    }

    pub fn switch(&self, account_id: i64) -> Result<AuthSession> {
        if let Some(session) = {
            let sessions = self.sessions.read();
            sessions.get(&account_id).cloned()
        } {
            self.store.set_last_active_account_id(Some(account_id))?;
            *self.active_id.write() = Some(account_id);
            return Ok(session);
        }

        let account = self
            .store
            .get_account_by_id(account_id)?
            .ok_or(SessionError::AccountNotFound)?;
        let token = self
            .store
            .get_token(account_id)?
            .ok_or(SessionError::TokenNotFound)?;
        let session = self.flow.resume(account.clone(), token)?;
        self.sessions.write().insert(account_id, session.clone());
        self.store.set_last_active_account_id(Some(account_id))?;
        *self.active_id.write() = Some(account_id);
        Ok(session)
    }

    pub fn begin_login(&self) -> Result<AuthorizationRequest> {
        self.flow.begin()
    }

    pub fn complete_login(&self, authz: AuthorizationRequest) -> Result<AuthSession> {
        let session = self.flow.complete(authz)?;
        self.sessions
            .write()
            .insert(session.account.id, session.clone());
        self.store
            .set_last_active_account_id(Some(session.account.id))?;
        *self.active_id.write() = Some(session.account.id);
        Ok(session)
    }

    pub fn active_token_provider(&self) -> Result<Arc<dyn TokenProvider>> {
        let active_id = self
            .active_account_id()
            .ok_or(SessionError::AccountNotFound)?;
        self.flow.token_provider(active_id)
    }

    pub fn token_provider(&self, account_id: i64) -> Result<Arc<dyn TokenProvider>> {
        if account_id == 0 {
            bail!(SessionError::AccountNotFound);
        }
        self.flow.token_provider(account_id)
    }
}
