use example::account::infra::security::bcrypt_hasher::BcryptHasherConfig;
use example::wire;

fn main() {
    let cfg = wire::Config {
        bcrypt: BcryptHasherConfig::default(),
    };
    let _ctx = wire::ServiceContext::new(&cfg);
    println!("Hello, world!");
}
