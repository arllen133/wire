use wire::{injectable, provider};

use crate::account::domain::security::PasswordHasher;
use std::io::Result;

#[provider(config("bcrypt"))]
#[derive(Clone, Debug, Default)]
pub struct BcryptHasherConfig {
    pub cost: u32,
}

#[derive(Clone)]
pub struct Connection {}

#[provider]
#[injectable]
pub struct BcryptHasher {
    // #[inject(cfg.default=12)]
    // cost: i32,
    #[inject]
    cfg: BcryptHasherConfig,

    #[inject(manual)]
    conn: Connection,
}

#[provider]
impl PasswordHasher for BcryptHasher {
    fn hash(&self, _raw_password: &str) -> Result<String> {
        todo!()
    }

    fn verify(&self, _raw_password: &str, _hashed_password: &str) -> Result<bool> {
        Ok(true)
    }
}
