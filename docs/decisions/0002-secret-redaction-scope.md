# 0002 — Where secrets are masked

## Status
Accepted (0.19.1).

## Context
SNMP community strings and SNMPv3 passwords travel through the service in
commands, logs, API responses and config files. The first plan was to mask
them everywhere.

## Decision
- **Logs and command traces: always masked** (`src/redact.rs`), at every log
  level including trace: the command line, the wire-level TX/RX dumps, the
  per-vendor "adding/removing community" messages, apply result actions and
  the diff debug output. Output of any `show ...` command is withheld from
  trace logs entirely, because wire chunks cut lines mid-way and a running
  config can contain secrets in any vendor's format. Nothing depends on a log
  containing a secret.
- **API responses that show the user's own authored config are not masked**:
  `desired-config`, `preview-diff`, `get_config.parsed_state` and
  `get_config.raw_config`. The UI manages communities by name (it lists them
  and removes them through the name in the URL), `parsed_state` is documented
  as directly comparable with `desired-config`, and `raw_config` is a
  restorable backup (`jq -r .raw_config > backup.conf`). Masking would break
  all three.
- **Secrets that come from files or systemd credentials** (planned
  `name_file` / `name_credential`) are protected in those responses by value:
  the resolved string is registered as a secret and scrubbed from API output,
  and only the reference is ever serialised or written to an overlay.

## Consequences
An inline `name:` community is as visible as the config file it is written in;
anyone who needs it kept out of the API should use a file or credential
source. The API is not authenticated, so it should not be reachable from
untrusted networks regardless.
