use example::wire;

fn main() {
    let dep = wire::Dependency {
        config: wire::Config::default(),
        connection: example::account::infra::security::bcrypt_hasher::Connection {},
    };
    wire::ServiceContext::new(&dep);
    println!("Hello, world!");
}
