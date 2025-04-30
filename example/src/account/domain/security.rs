use std::io::Result;
pub trait PasswordHasher: Send + Sync {
    /// 哈希密码
    fn hash(&self, raw_password: &str) -> Result<String>;

    /// 校验密码与哈希是否匹配
    fn verify(&self, raw_password: &str, hashed_password: &str) -> Result<bool>;
}
