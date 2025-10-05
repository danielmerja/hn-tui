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
                    "Reddix — Reddit, refined for the terminal.\n\n  --version, -V        Show version and exit\n  --help,    -h        Show this help message\n  --check-updates      Check for updates and exit"
                );
                saw_flag = true;
            }
            "--check-updates" => {
                saw_flag = true;
                if let Err(err) = check_updates_once() {
                    eprintln!("Update check failed: {err:?}");
                    std::process::exit(1);
                }
            }
            _ => {}
        }
    }
    saw_flag
}

fn check_updates_once() -> anyhow::Result<()> {
    use semver::Version;

    let skip_env = reddix::update::SKIP_UPDATE_ENV;
    if std::env::var(skip_env).is_ok() {
        println!("Update check skipped: {skip_env} is set.");
        return Ok(());
    }

    let current = Version::parse(reddix::VERSION)?;
    match reddix::update::check_for_update(&current)? {
        Some(info) => {
            let reddix::update::UpdateInfo { version, url } = info;
            println!("Update available: {current} -> {version}\n{url}");
        }
        None => {
            println!("Reddix {current} is up to date.");
        }
    }
    Ok(())
}
