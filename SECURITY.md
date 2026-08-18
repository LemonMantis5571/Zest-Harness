# Security policy

## Supported versions

Security fixes target the latest published Zest beta. Older beta builds may
not receive fixes; update before reporting a vulnerability against a current
release.

## Reporting a vulnerability

Please use a private
[GitHub Security Advisory](https://github.com/LemonMantis5571/Zest/security/advisories/new)
for reports. Do not open a public issue for an unpatched vulnerability.

Include:

- the affected Zest version or commit;
- the operating system and whether the installer or source build was used;
- a minimal reproduction and its security impact; and
- any logs with API keys, credentials, personal data, or workspace contents
  removed.

Never include an API key, credential-manager export, private key,
or full workspace archive in a report. If a key may have been exposed, revoke
it with the provider before investigating further.

## Security boundaries

Zest keeps OpenAI-compatible provider keys entered through the desktop in the OS
credential manager and does not intentionally write provider secrets to
`zest.toml`, the transcript, tool context, logs, or telemetry. Native Anthropic
keys are supplied through the environment variable named by `api_key_env`.
The approval system is a user confirmation boundary, not an OS sandbox: an
approved shell command can perform any action available to the current user.
Use isolated or disposable workspaces for untrusted code.

The Linux beta uses the GTK3 stack required by the current Tauri runtime.
RustSec may report transitive unmaintained GTK crates and the known `glib`
unsoundness advisory. These are tracked beta dependency risks, not evidence
that the runtime is safe for hostile content; do not use the beta as a sandbox.
