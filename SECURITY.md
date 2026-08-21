# Security Policy

Please **do not** open a public GitHub issue for a vulnerability.

## Reporting

Use GitHub [private vulnerability reporting](https://github.com/DR4G4NS/Oxide/security/advisories/new) (Settings → Code security, once the repo is public).

Include:

- Oxide version or git commit
- What happens vs what you expected
- A minimal repro (flags, map if you can share one, client build)

We will acknowledge the report and work on a fix before any public write-up.

## Scope

**In scope:** remote crashes, unauthorized admin, wire or world-state corruption a vanilla 159.7 client can trigger, secrets in logs.

**Out of scope:** missing vanilla features already listed in the README, client-side bugs, filling the server with legitimate players up to `--max-players`.

## Supported versions

Only the latest `0.1.x-alpha` on `main` receives security fixes.
