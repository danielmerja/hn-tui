fn main() {
    if handle_cli_flags() {
        return;
    }

    if let Err(err) = reddix::run() {
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
}

fn handle_cli_flags() -> bool {
    let mut saw_flag = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("Reddix {}", reddix::VERSION);
                saw_flag = true;
            }
            "--help" | "-h" => {
                println!(
                    "Reddix — Reddit, refined for the terminal.\n\n  --version, -V    Show version and exit\n  --help,    -h    Show this help message"
                );
                saw_flag = true;
            }
            _ => {}
        }
    }
    saw_flag
}
