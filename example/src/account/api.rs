use std::sync::Arc;
use wire::injectable;

use crate::account::domain::security::PasswordHasher;

#[injectable(export("account_grpc_service"))]
pub struct AccountGrpcServer {
    #[inject]
    password_hasher: Arc<dyn PasswordHasher>,
}
