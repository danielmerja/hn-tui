# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
- Nothing yet.

## [0.1.0] - 2025-10-29
### Added
- Initial HN-TUI release, forked from Reddix and refactored for Hacker News.
- Support for HN story categories: Top, New, Best, Ask HN, Show HN, and Jobs.
- Full comment threading with recursive fetching from HN Firebase API.
- ASCII fallback icons via `HN_TUI_DISABLE_NERD_FONTS` environment variable.
- Updated all branding from Reddix to HN-TUI.

### Changed
- Replaced Reddit OAuth authentication with public Hacker News Firebase API (no auth required).
- Simplified app initialization by removing OAuth flow.
- Updated configuration paths from `reddix` to `hn-tui`.
- Changed environment variables from `REDDIX_*` to `HN_TUI_*`.
- Updated UI strings to reflect HN terminology (categories instead of subreddits).

### Removed
- Voting functionality (HN API is read-only without authentication).
- Commenting functionality (HN API is read-only without authentication).
- Reddit-specific features (subscriptions, galleries, etc.).

## Previous Reddix History

This project was forked from [Reddix](https://github.com/ck-zhang/reddix) v0.2.5. 
For the complete history of the original Reddix project, see the [Reddix repository](https://github.com/ck-zhang/reddix/blob/main/CHANGELOG.md).

Key features inherited from Reddix:
- Terminal UI with three-pane layout
- Inline media previews with Kitty graphics protocol support
- Video playback via mpv integration
- Keyboard-driven navigation
- Full-resolution media saving
- Help overlay and guided menu

[Unreleased]: https://github.com/danielmerja/hn-tui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/danielmerja/hn-tui/releases/tag/v0.1.0
