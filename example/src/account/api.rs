use std::sync::Arc;
use wire::injectable;

use crate::account::domain::security::PasswordHasher;

#[injectable]
pub struct AccountGrpcServer {
    #[inject]
    password_hasher: Arc<dyn PasswordHasher>,
}
