pub mod account;

pub mod wire {
    include!(concat!(env!("OUT_DIR"), "/wire.rs"));
}
