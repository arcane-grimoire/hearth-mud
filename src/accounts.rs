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
    pub email: Option<String>,
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
            // Assigned a real dbref on first login — see Engine::enter_world.
            character_ref: None,
            email: None,
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

    pub fn set_email(&mut self, account_id: &str, email: Option<String>) -> Result<(), String> {
        let account = self
            .accounts
            .get_mut(account_id)
            .ok_or("Account not found.")?;
        if let Some(ref e) = email {
            if !e.contains('@') || !e.contains('.') {
                return Err("That doesn't look like a valid email address.".into());
            }
        }
        account.email = email;
        Ok(())
    }

    pub fn change_password(
        &mut self,
        account_id: &str,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), String> {
        let account = self
            .accounts
            .get(account_id)
            .ok_or("Account not found.")?;
        if !verify_password(old_password, &account.password_hash) {
            return Err("Incorrect current password.".into());
        }
        if new_password.len() < 6 {
            return Err("New password must be at least 6 characters.".into());
        }
        let hash = hash_password(new_password)?;
        self.accounts.get_mut(account_id).unwrap().password_hash = hash;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_account() {
        let mut store = AccountStore::new();
        let account = store.create("TestUser", "password123").unwrap();
        assert_eq!(account.username, "TestUser");
        assert_eq!(account.character_ref, None);
    }

    #[test]
    fn first_account_is_admin() {
        let mut store = AccountStore::new();
        let account = store.create("admin", "password123").unwrap();
        assert!(account.scopes.contains(&Scope::Admin));
        assert!(account.scopes.contains(&Scope::Builder));
    }

    #[test]
    fn second_account_is_player_only() {
        let mut store = AccountStore::new();
        store.create("first", "password123").unwrap();
        let second = store.create("second", "password123").unwrap();
        assert!(second.scopes.contains(&Scope::Player));
        assert!(!second.scopes.contains(&Scope::Admin));
        assert!(!second.scopes.contains(&Scope::Builder));
    }

    #[test]
    fn authenticate_success() {
        let mut store = AccountStore::new();
        store.create("alice", "hunter42").unwrap();
        assert!(store.authenticate("alice", "hunter42").is_ok());
    }

    #[test]
    fn authenticate_wrong_password() {
        let mut store = AccountStore::new();
        store.create("alice", "hunter42").unwrap();
        assert!(store.authenticate("alice", "wrong").is_err());
    }

    #[test]
    fn authenticate_case_insensitive_username() {
        let mut store = AccountStore::new();
        store.create("Alice", "hunter42").unwrap();
        assert!(store.authenticate("ALICE", "hunter42").is_ok());
        assert!(store.authenticate("alice", "hunter42").is_ok());
    }

    #[test]
    fn duplicate_username_rejected() {
        let mut store = AccountStore::new();
        store.create("alice", "password123").unwrap();
        assert!(store.create("alice", "password456").is_err());
        assert!(store.create("ALICE", "password456").is_err());
    }

    #[test]
    fn short_username_rejected() {
        let mut store = AccountStore::new();
        assert!(store.create("ab", "password123").is_err());
    }

    #[test]
    fn short_password_rejected() {
        let mut store = AccountStore::new();
        assert!(store.create("alice", "short").is_err());
    }

    #[test]
    fn change_password() {
        let mut store = AccountStore::new();
        let id = store.create("alice", "oldpass123").unwrap().id.clone();
        assert!(store.change_password(&id, "oldpass123", "newpass456").is_ok());
        assert!(store.authenticate("alice", "newpass456").is_ok());
        assert!(store.authenticate("alice", "oldpass123").is_err());
    }

    #[test]
    fn change_password_wrong_old() {
        let mut store = AccountStore::new();
        let id = store.create("alice", "oldpass123").unwrap().id.clone();
        assert!(store.change_password(&id, "wrong", "newpass456").is_err());
    }

    #[test]
    fn grant_and_revoke_scope() {
        let mut store = AccountStore::new();
        let id = store.create("first", "password123").unwrap().id.clone();
        store.create("second", "password123").unwrap();
        let second_id = store.get_id_by_username("second").unwrap();
        store.grant_scope(&second_id, Scope::Builder);
        assert!(store.get(&second_id).unwrap().has_scope(Scope::Builder));
        store.revoke_scope(&second_id, Scope::Builder);
        assert!(!store.get(&second_id).unwrap().scopes.contains(&Scope::Builder));
    }
}
