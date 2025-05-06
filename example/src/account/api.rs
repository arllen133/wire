use serde::Deserialize;
use std::sync::Arc;
use wire::{injectable, provider};

use crate::account::domain::security::PasswordHasher;

pub trait Hello {}

#[provider(config("account"))]
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AccountConfig {
    pub addr: String,
    pub port: u16,
}

#[allow(dead_code)]
#[injectable(export, rename("account_grpc_service"))]
pub struct AccountGrpcServer {
    #[inject]
    config: AccountConfig,
    #[inject]
    password_hasher: Arc<dyn PasswordHasher>,
}
