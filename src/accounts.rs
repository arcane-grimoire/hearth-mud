use std::collections::{HashMap, HashSet};

use argon2::Argon2;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Player,
    Builder,
    Admin,
}

impl Scope {
    pub fn label(&self) -> &'static str {
        match self {
            Scope::Player => "player",
            Scope::Builder => "builder",
            Scope::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "player" => Some(Scope::Player),
            "builder" => Some(Scope::Builder),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub character_ref: Option<String>,
    pub scopes: HashSet<Scope>,
}

impl Account {
    pub fn has_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&Scope::Admin) || self.scopes.contains(&scope)
    }

    pub fn is_admin(&self) -> bool {
        self.scopes.contains(&Scope::Admin)
    }

    pub fn is_builder(&self) -> bool {
        self.has_scope(Scope::Builder)
    }

    pub fn scope_labels(&self) -> Vec<&'static str> {
        let mut labels: Vec<_> = self.scopes.iter().map(|s| s.label()).collect();
        labels.sort();
        labels
    }
}

pub struct AccountStore {
    accounts: HashMap<String, Account>,
    by_username: HashMap<String, String>,
}

impl AccountStore {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            by_username: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn create(&mut self, username: &str, password: &str) -> Result<&Account, String> {
        let lower = username.to_lowercase();
        if self.by_username.contains_key(&lower) {
            return Err("That username is already taken.".into());
        }
        if username.len() < 3 {
            return Err("Username must be at least 3 characters.".into());
        }
        if username.len() > 20 {
            return Err("Username must be 20 characters or fewer.".into());
        }
        if !username.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err("Username may only contain letters, numbers, and underscores.".into());
        }
        if password.len() < 6 {
            return Err("Password must be at least 6 characters.".into());
        }

        let hash = hash_password(password)?;
        let id = Uuid::new_v4().to_string();
        let character_ref = format!("player/{}", lower);

        // First account gets admin
        let scopes = if self.accounts.is_empty() {
            HashSet::from([Scope::Player, Scope::Builder, Scope::Admin])
        } else {
            HashSet::from([Scope::Player])
        };

        let account = Account {
            id: id.clone(),
            username: username.to_string(),
            password_hash: hash,
            character_ref: Some(character_ref),
            scopes,
        };

        self.accounts.insert(id.clone(), account);
        self.by_username.insert(lower, id.clone());
        Ok(self.accounts.get(&id).unwrap())
    }

    pub fn authenticate(&self, username: &str, password: &str) -> Result<&Account, String> {
        let lower = username.to_lowercase();
        let id = self
            .by_username
            .get(&lower)
            .ok_or("Invalid username or password.")?;
        let account = self.accounts.get(id).unwrap();

        if verify_password(password, &account.password_hash) {
            Ok(account)
        } else {
            Err("Invalid username or password.".into())
        }
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.get_mut(id)
    }

    pub fn get_by_username(&self, username: &str) -> Option<&Account> {
        let lower = username.to_lowercase();
        self.by_username
            .get(&lower)
            .and_then(|id| self.accounts.get(id))
    }

    pub fn get_id_by_username(&self, username: &str) -> Option<String> {
        let lower = username.to_lowercase();
        self.by_username.get(&lower).cloned()
    }

    pub fn insert(&mut self, account: Account) {
        let lower = account.username.to_lowercase();
        self.by_username.insert(lower, account.id.clone());
        self.accounts.insert(account.id.clone(), account);
    }

    pub fn all(&self) -> impl Iterator<Item = &Account> {
        self.accounts.values()
    }

    pub fn grant_scope(&mut self, account_id: &str, scope: Scope) -> bool {
        if let Some(account) = self.accounts.get_mut(account_id) {
            account.scopes.insert(scope);
            true
        } else {
            false
        }
    }

    pub fn revoke_scope(&mut self, account_id: &str, scope: Scope) -> bool {
        if let Some(account) = self.accounts.get_mut(account_id) {
            account.scopes.remove(&scope);
            true
        } else {
            false
        }
    }
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("Failed to hash password: {}", e))
}

fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}
