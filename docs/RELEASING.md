# Release Process

This document explains how to create a new release of hn-tui using the automated GitHub Actions workflow.

## Overview

The release workflow is powered by [cargo-dist](https://github.com/axodotdev/cargo-dist) and automatically:
- Builds binaries for multiple platforms (Linux, macOS, Windows)
- Creates installers (shell and PowerShell scripts)
- Generates checksums and signatures
- Creates a GitHub Release with all artifacts
- Generates release notes from the CHANGELOG

## Prerequisites

1. **GitHub Actions must be enabled** in your repository
2. **Workflow permissions** must be set to allow writing:
   - Go to repository Settings → Actions → General
   - Under "Workflow permissions", select "Read and write permissions"
   - Ensure "Allow GitHub Actions to create and approve pull requests" is checked
3. **Update the CHANGELOG.md** with the new version's changes

## Creating a Release

### 1. Update Version and CHANGELOG

First, update the version in `Cargo.toml` and `CHANGELOG.md`:

```bash
# Update version in Cargo.toml
# Change: version = "0.1.0"
# To:     version = "0.2.0"

# Update CHANGELOG.md to move items from [Unreleased] to new version
# Add a new section like:
## [0.2.0] - 2025-10-30
### Added
- New feature descriptions...
```

Commit these changes:

```bash
git add Cargo.toml CHANGELOG.md Cargo.lock
git commit -m "Bump version to 0.2.0"
git push
```

### 2. Create and Push a Version Tag

The workflow is triggered by pushing a git tag that matches the version pattern `[0-9]+.[0-9]+.[0-9]+`:

```bash
# Create a tag (with 'v' prefix is conventional but optional)
git tag v0.2.0

# Push the tag to GitHub
git push origin v0.2.0
```

**Supported tag formats:**
- `v0.2.0` (recommended)
- `0.2.0`
- `v0.2.0-beta.1` (for prereleases)
- `0.2.0-rc.1` (for release candidates)

### 3. Monitor the Release Workflow

1. Go to your repository on GitHub
2. Click the "Actions" tab
3. You should see a new "Release" workflow running
4. The workflow typically takes 10-20 minutes to complete

The workflow will:
- Build for all platforms in parallel
- Run tests (if configured)
- Create installers
- Upload all artifacts to a new GitHub Release

### 4. Verify the Release

Once the workflow completes successfully:

1. Go to the "Releases" page in your repository
2. You should see a new release with:
   - Release notes generated from CHANGELOG.md
   - Binary archives for each platform:
     - `hn-tui-x86_64-unknown-linux-gnu.tar.gz`
     - `hn-tui-x86_64-apple-darwin.tar.gz`
     - `hn-tui-aarch64-apple-darwin.tar.gz`
     - `hn-tui-x86_64-pc-windows-msvc.zip`
     - etc.
   - Installer scripts (shell and PowerShell)
   - Checksums file

## Platform Support

The release workflow builds for these platforms:
- **Linux**: x86_64, aarch64 (ARM64)
- **macOS**: x86_64 (Intel), aarch64 (Apple Silicon)
- **Windows**: x86_64

## Troubleshooting

### Workflow doesn't trigger

- **Check workflow permissions**: Ensure Actions have write permissions (see Prerequisites)
- **Verify tag format**: Tags must match pattern `[0-9]+.[0-9]+.[0-9]+` (e.g., `v0.2.0`, `0.2.0`)
- **Check Actions are enabled**: Repository Settings → Actions → Allow all actions

### Workflow fails

1. Click on the failed workflow run in the Actions tab
2. Check the logs for specific error messages
3. Common issues:
   - **Build failures**: Fix any compilation errors first
   - **Test failures**: Ensure all tests pass locally with `cargo test`
   - **Permission errors**: Check workflow permissions in repository settings

### Need to fix a failed release

1. Delete the failed tag and release:
   ```bash
   # Delete local tag
   git tag -d v0.2.0
   
   # Delete remote tag
   git push --delete origin v0.2.0
   
   # Delete the release on GitHub (manually via the web UI)
   ```

2. Fix the issues in your code
3. Commit and push the fixes
4. Re-create and push the tag

## Alternative: Local Builds

If you need to build releases locally (e.g., for testing or if the workflow isn't working):

```bash
# Install cargo-dist
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/axodotdev/cargo-dist/releases/download/v0.30.0/cargo-dist-installer.sh | sh

# Build for your current platform
cargo build --release --profile dist

# Or build with cargo-dist for cross-platform
dist build --artifacts=local
```

However, **using the GitHub Actions workflow is strongly recommended** as it:
- Builds for all platforms automatically
- Ensures reproducible builds
- Handles code signing and verification
- Creates installers automatically
- Publishes releases with proper artifacts

## Questions?

If you encounter issues not covered here:
1. Check the [cargo-dist documentation](https://axodotdev.github.io/cargo-dist/)
2. Review the workflow logs in the Actions tab
3. Open an issue in the repository
