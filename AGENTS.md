# AGENTS.md

## Release & Tagging

- **Before creating or pushing any git tag** (e.g. `vX.Y.Z`), you **must** update `docs/CHANGELOG.md` first:
  - Move or summarize the `## [Unreleased]` changes into a new `## [vX.Y.Z] - YYYY-MM-DD` section matching the exact tag name.
  - Commit and push the `docs/CHANGELOG.md` update before tagging, because `.github/workflows/release.yml` extracts GitHub Release notes directly from `docs/CHANGELOG.md` for the pushed tag.
