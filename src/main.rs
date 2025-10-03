fn main() {
    if let Err(err) = reddix::run() {
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
}
