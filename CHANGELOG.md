# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
- Nothing yet.

## [0.1.16] - 2025-10-08
### Added
- Clipboard shortcut (`y`) copies the highlighted comment to the system clipboard.

## [0.1.15] - 2025-10-08
### Added
- Comment sorting controls that mirror the post sort workflow, with a top-of-pane picker and `t` shortcut.

## [0.1.14] - 2025-10-07
### Added
- Command palette with fuzzy subreddit/user search and consolidated actions menu access.
- Full-resolution media saver (images only) that queues downloads without blocking the UI.
### Changed
- Navigation keys now respect typing mode so overlays no longer hijack text input.

## [0.1.13] - 2025-10-07
### Changed
- Wrapped subreddit sort shortcuts so the navigation pane stays readable on narrow terminals.

## [0.1.12] - 2025-10-07
### Changed
- Kept the selected subreddit centered by auto-scrolling the navigation list.

## [0.1.11] - 2025-10-06
### Added
- One-click updater that downloads and runs the latest installer from the banner.
### Changed
- Guided setup copy now highlights the example `config.yaml` and refreshed quick-start instructions.

## [0.1.10] - 2025-10-06
### Changed
- Simplified the guided authorization flow and trimmed conflicting shortcuts in the menu.
### Added
- Feature request tracker to capture community ideas in one place.

## [0.1.9] - 2025-10-06
### Fixed
- Restored `q` as the quit shortcut in the credentials form without blocking text entry.

## [0.1.8] - 2025-10-06
### Changed
- Resolved the remaining guided-menu shortcut clashes and refreshed the README preview image.

## [0.1.7] - 2025-10-06
### Fixed
- Cleared stale kitty previews when scrolling so inline media no longer leaves artifacts.

## [0.1.6] - 2025-10-06
### Changed
- Polished the update banner with clearer messaging, better selection defaults, and smoother post focus.

## [0.1.5] - 2025-10-05
### Added
- `--version`/`--help` flags with tests plus environment overrides to simulate update scenarios.
### Changed
- Kept the update banner visible even when the post list recenters.

## [0.1.4] - 2025-10-05
### Added
- Asynchronous subreddit refresh on login so the navigation list is ready sooner.
### Changed
- Streamlined the one-click join flow for `r/ReddixTUI` with clearer status messaging.

## [0.1.3] - 2025-10-05
### Added
- GitHub-backed update checker, in-app banner, and subreddit subscription helpers.
### Changed
- Refreshed README copy and screenshots to match the guided setup.

## [0.1.2] - 2025-10-04
### Added
- Enabled cargo-dist shell installers in the release workflow.

## [0.1.1] - 2025-10-04
### Added
- Persisted media previews so cached thumbnails load instantly.

## [0.1.0] - 2025-10-04
### Added
- Initial release with the polished login workflow, refreshed caching, and improved feed pagination.

[Unreleased]: https://github.com/ck-zhang/reddix/compare/v0.1.16...HEAD
[0.1.16]: https://github.com/ck-zhang/reddix/compare/v0.1.15...v0.1.16
[0.1.15]: https://github.com/ck-zhang/reddix/compare/v0.1.14...v0.1.15
[0.1.14]: https://github.com/ck-zhang/reddix/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/ck-zhang/reddix/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/ck-zhang/reddix/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/ck-zhang/reddix/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/ck-zhang/reddix/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/ck-zhang/reddix/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/ck-zhang/reddix/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/ck-zhang/reddix/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/ck-zhang/reddix/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ck-zhang/reddix/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ck-zhang/reddix/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ck-zhang/reddix/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ck-zhang/reddix/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ck-zhang/reddix/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ck-zhang/reddix/releases/tag/v0.1.0
