fn main() {
    if handle_cli_flags() {
        return;
    }

    if let Err(err) = hn_tui::run() {
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
}

fn handle_cli_flags() -> bool {
    let mut saw_flag = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("HN-TUI {}", hn_tui::VERSION);
                saw_flag = true;
            }
            "--help" | "-h" => {
                println!(
                    "HN-TUI — Browse Hacker News from the terminal.\n\n  --version, -V        Show version and exit\n  --help,    -h        Show this help message\n  --check-updates      Check for updates and exit"
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

    let skip_env = hn_tui::update::SKIP_UPDATE_ENV;
    if std::env::var(skip_env).is_ok() {
        println!("Update check skipped: {skip_env} is set.");
        return Ok(());
    }

    let current = Version::parse(hn_tui::VERSION)?;
    match hn_tui::update::check_for_update(&current)? {
        Some(info) => {
            let hn_tui::update::UpdateInfo {
                version,
                release_url,
                ..
            } = info;
            println!("Update available: {current} -> {version}\n{release_url}");
        }
        None => {
            println!("HN-TUI {current} is up to date.");
        }
    }
    Ok(())
}
