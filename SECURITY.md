# Security Policy

## Supported versions

Security fixes are provided for the latest 1.x release.

| Version | Supported |
|---|---|
| 1.0.x | Yes |
| < 1.0 | No |

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.**

Preferred: open a private report through GitHub Security Advisories at
<https://github.com/PizenLabs/gry/security/advisories/new>. This keeps the
report, discussion, and fix private until a coordinated disclosure.

If you cannot use GitHub Security Advisories, email **contact@pizenlabs.dev** with
the subject line `gry security`. Encrypt sensitive details if possible, and
include a way to contact you securely.

Please include as much of the following as you can:

- the affected version (`gry --version`) and platform;
- a description of the issue and its impact;
- reproduction steps or a minimal proof of concept;
- any relevant configuration, MCP client, or repository characteristics;
- whether the issue has been disclosed elsewhere.

## What to expect

- We aim to acknowledge a report within **3 business days**.
- We will investigate, confirm the impact, and agree on a disclosure timeline
  with you.
- We will credit reporters in the advisory unless you prefer to remain
  anonymous.
- We aim to ship a fix and publish an advisory within **90 days** of a
  confirmed report. If a fix takes longer, we will explain why and keep you
  updated.

Please give us a reasonable opportunity to fix the issue before publishing
details. We support coordinated disclosure and will not pursue legal action
against researchers who act in good faith and follow this policy.

## Scope

gry runs locally, reads repository content, stores a SQLite database, and
communicates with MCP clients over stdio. Areas of particular interest include:

- memory-safety, panic, or denial-of-service issues in parsers, extractors, or
  language fallbacks when processing malicious repository content;
- path traversal, symlink handling, or writes outside the configured cache or
  `GRY_ALLOWED_ROOT` boundary;
- SQL injection, unsafe deserialization, or corruption of the local database;
- JSON-RPC/MCP input handling, pagination, and output-budget bypasses;
- unsafe handling of client configuration files by `gry install`/`gry uninstall`;
- vulnerabilities in bundled dependencies or the bundled SQLite build;
- leakage of indexed source or configuration data beyond the local machine.

The following are generally out of scope:

- issues that require an attacker who already has the same local privileges as
  the user running gry;
- vulnerabilities in third-party MCP clients, editors, or agents;
- social engineering, spam, or physical attacks;
- reports that only affect unsupported versions or local modifications;
- findings that depend on disabling documented security controls without a
  bypass of those controls.

## Safe harbor

We consider security research conducted in accordance with this policy to be
authorized and beneficial to the community. We will not initiate or support
legal action against researchers for accidental, good-faith violations of this
policy, provided they:

- avoid privacy violations, data destruction, and service disruption;
- only test against systems and repositories they own or have permission to
  test;
- report findings privately and give us time to respond before disclosure.

## Security updates

Security advisories are published through
[GitHub Security Advisories](https://github.com/PizenLabs/gry/security/advisories).
Release notes in [CHANGELOG.md](CHANGELOG.md) call out security-relevant fixes.
