# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added
- Rich milestone and issue lifecycle support in the CLI, including:
  - Extended `git mile create` flags for descriptions, initial comments, and labels.
  - `git mile comment` helpers with quoting, editor templates, and JSON output.
  - `git mile label` add/remove/set flows with JSON summaries.
  - Enhanced `git mile list` (column selection, long view, JSON payloads) and
    `git mile show` (Markdown-aware rendering, comment limits) aligned with the
    new core services.
- Shared core models and services for descriptions, comments, and label history
  to power the enriched CLI experience.
- Documentation updates:
  - CLI command reference describing the new flags and outputs.
  - Lifecycle guide demonstrating end-to-end milestone and issue workflows.
- Automated tests covering comment and label persistence at both the core and
  CLI levels.

### Changed
- Updated README to surface the enriched command set and new documentation.

