fn main() {
    eprintln!("=====> building...");
    wire_build::configure().parse_dir("src".to_string()).build();
}
