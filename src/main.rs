// The library crate is compiled as "host"
extern crate host;

fn main() {
    std::process::exit(host::rust_main());
}
