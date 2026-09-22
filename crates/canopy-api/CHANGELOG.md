# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.0.3](https://github.com/beyondessential/canopy/compare/bes-canopy-api-v1.0.2...bes-canopy-api-v1.0.3) - 2026-09-22

### Other

- parse and format what the generator emits
- keep the payload plumbing in the crate
- refuse a path value that resolves a segment away
- refuse a path value that would change which request is made
- carry binary and text bodies in the generated client

## [1.0.2](https://github.com/beyondessential/canopy/compare/bes-canopy-api-v1.0.1...bes-canopy-api-v1.0.2) - 2026-09-20

### Other

- Merge branch 'feat/group-scoped-artifacts' into feat/reporting-schemas
- Merge branch 'epic/deployment-artefacts' into feat/group-scoped-artifacts

## [1.0.1](https://github.com/beyondessential/canopy/compare/bes-canopy-api-v1.0.0...bes-canopy-api-v1.0.1) - 2026-09-14

### Fixed

- *(ci)* release-plz manages only the API client; deploy every push again

### Other

- correct the error field contract
- store migration error

## [1.0.0](https://github.com/beyondessential/canopy/releases/tag/bes-canopy-api-v1.0.0) - 2026-09-04

Initial release.
