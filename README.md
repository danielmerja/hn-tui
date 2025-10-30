# HN-TUI

[![Release](https://img.shields.io/github/v/release/danielmerja/hn-tui?style=flat-square)](https://github.com/danielmerja/hn-tui/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square)](LICENSE)

HN-TUI - Browse Hacker News from the terminal.

## Features

- Browse Hacker News stories (Top, New, Best, Ask HN, Show HN, Jobs)
- Read comments with threaded display
- Keyboard-first navigation
- No authentication required (HN API is public)
- Terminal-native interface using Ratatui
- Smart caching

## Install

### Binary Releases

Download pre-built binaries for Linux, macOS, or Windows from the [latest release](https://github.com/danielmerja/hn-tui/releases/latest).

Quick install with shell script (Linux/macOS):
```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/danielmerja/hn-tui/releases/latest/download/hn-tui-installer.sh | sh
```

Or with PowerShell (Windows):
```powershell
irm https://github.com/danielmerja/hn-tui/releases/latest/download/hn-tui-installer.ps1 | iex
```

### From Source

```sh
git clone https://github.com/danielmerja/hn-tui
cd hn-tui
cargo build --release
./target/release/hn-tui
```

### Using Cargo

```sh
cargo install --git https://github.com/danielmerja/hn-tui
```

## Usage

Simply run:

```sh
hn-tui
```

No configuration or authentication needed - the Hacker News API is completely public.

### Keyboard Shortcuts

- `j/k` - Navigate up/down in lists
- `h/l` - Switch between panes (categories/stories/content)
- `Enter` - View story or open comments
- `p` - Refresh current view
- `q` - Quit

## Configuration

Configuration file is optional and located at `~/.config/hn-tui/config.yaml`.

You can customize:
- UI theme
- Cache settings
- Media preview settings

### Environment Variables

- `HN_TUI_DISABLE_NERD_FONTS=1` - Use ASCII fallback icons instead of Nerd Font icons (helpful if icons appear as boxes or question marks)

## About

HN-TUI is a fork of [Reddix](https://github.com/ck-zhang/reddix), refactored to work with Hacker News instead of Reddit.

Key changes:
- Replaced Reddit OAuth with Hacker News Firebase API
- Removed authentication (HN is public)
- Adapted UI for HN story categories
- Simplified configuration

## License

MIT License - see [LICENSE](LICENSE) for details

## Original Credits

This project is based on Reddix by ck-zhang. Original concept and UI design credit goes to the Reddix project.
