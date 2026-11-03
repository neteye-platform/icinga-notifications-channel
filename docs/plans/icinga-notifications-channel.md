# Plan: `icinga-notifications-channel`

A library for writing [Icinga Notifications channel plugins][channels] on top of
`jsonrpc-rs`. The Go [`plugin` package][go-plugin] is the reference implementation.

Each step ends in a state where `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features -- -D warnings` and
`cargo test --all-targets --all-features` pass. A step only depends on earlier
steps; its tests do not need any later step.

[channels]: https://github.com/Icinga/icinga-notifications/blob/main/doc/10-Channels.md
[go-plugin]: https://pkg.go.dev/github.com/icinga/icinga-go-library/notifications/plugin

## Decisions

- `Channel::Config: ChannelConfig`; `GetInfo.config_attrs` is generated from it.
- Config is stored as `RwLock<Option<Arc<Config>>>`. `SendNotification` clones the
  `Arc` under a short read lock, so a slow send never blocks `SetConfig`.
- Before the first `SetConfig`, `SendNotification` fails with `-32000`.
- One writer task owns stdout. Responses and `channel::Log` notifications reach it
  through one mpsc channel.
- `run` stops on stdin EOF, SIGINT or SIGTERM, then drains running requests for at
  most `RunOptions::drain_timeout` (default 5 s).
- Translations: one TOML file per locale, read and validated by the derive at compile
  time and embedded in the binary. `en_US.toml` is required and must have a `label`
  for every field.
- Defaults come from serde (`#[serde(default)]`, `#[serde(default = "f")]`), not
  from a separate attribute.
- Out of scope: secret placeholders, a compile-time completeness check for
  non-`en_US` locales.

## Crates

| Crate | Contents |
| --- | --- |
| `icinga-notifications-channel` | Runtime, traits, protocol types, `Secret` |
| `icinga-notifications-channel-macros` | `ChannelConfig` and `ConfigEnum` derives |

The runtime re-exports the derives behind a default-on `derive` feature.

---

## Phase 1: Protocol types (no IO)

### 1. Crate skeleton

- Add `icinga-notifications-channel` to the workspace, depending on `jsonrpc-rs`,
  `serde`, `serde_json`, `tokio`, `tracing`.
- **Test:** `cargo build -p icinga-notifications-channel`.

### 2. `Localized` and `ConfigType`

- `Localized`: a `BTreeMap<Locale, String>` that can only be built with an `en_US`
  entry. `Locale` validates `language_REGION`.
- `ConfigType`: `String, Number, Bool, Text, Option, Options, Secret`, serialized as
  the Go strings (`string`, `number`, `bool`, `text`, `option`, `options`, `secret`).
- **Test (unit):** a missing `en_US` is rejected; an invalid locale such as `en-US`
  is rejected with a message naming it; serialization matches the Go names.

### 3. `ConfigOption` builder

- `ConfigOption::new(name, ConfigType, Localized)` with `.help()`, `.default()`,
  `.required()`, `.min()`, `.max()`, `.options()`, `.child()`.
- `min`/`max` only for `Number`, `options` only for `Option`/`Options`; children
  cannot have children. Enforced through the types, otherwise rejected by the builder.
- `ChildOption` with `parent_values`.
- **Test (unit):** serialize the two `config_attrs` entries from the doc's `GetInfo`
  example and compare as `serde_json::Value`, including `"min": null, "max": null`.

### 4. `Info` and `GetInfo` response

- `Info { name, version, author }` and a private `GetInfoResponse` that adds
  `supported_request_versions: [1]` and `config_attrs`.
- **Test (unit):** the doc's `GetInfo` response example round-trips.

### 5. Notification request types

- `NotificationRequest { version, payload: Box<RawValue> }`.
- `NotificationV1`, `Contact`, `Address`, `Object`, `Incident`, `Event`, `Severity`,
  with `incident: Option<Incident>` and `state: Option<State>`.
- Timestamps: RFC 3339 strings parsed into a time type (pick `jiff` or `time`; prefer
  whichever the workspace already uses, otherwise `jiff`).
- **Test (unit):** the doc's `SendNotification` example deserializes; a payload with
  `"incident": null` and no `state` deserializes.

### 6. State

- `StateKey` (opaque, no public constructor), `State { key, value }`.
- `State::update(value) -> Result<StateUpdate, ChannelError>`: value length 1..=4096
  characters, checked once here.
- **Test (unit):** 0 and 4097 characters fail with a message giving the length and
  the allowed range; 1 and 4096 succeed; the key in `StateUpdate` is the received key.

### 7. Delivery result

- `DeliveryDetail` enum: `Number, Boolean, Severity, Text, Url { url: HttpUrl,
  anchor_text }, Datetime`, serialized as `{type, label, value, anchor_text?}`.
- `HttpUrl` accepts only absolute `http`/`https` URLs.
- `DeliveryResult { state: Option<StateUpdate>, details: Vec<DeliveryDetail> }`.
- **Test (unit):** the doc's `SendNotification` response example serializes;
  `ftp://x` and `/relative` are rejected by `HttpUrl`.

### 8. `ChannelError`

- `ChannelError::new(message)` (`-32000`), `ChannelError::with_code`, and
  `From<ChannelError> for JsonRpcResult`.
- **Test (unit):** conversion produces the expected error object.

## Phase 2: Runtime

### 9. `Channel` and `ChannelConfig` traits

- `ChannelConfig: DeserializeOwned + Send + Sync + 'static { fn config_attrs() ->
  Vec<ConfigOption>; }`.
- `Channel { type Config: ChannelConfig; fn info(&self) -> Info; fn validate(...);
  fn send_notification(...) -> impl Future<...> + Send; }`.
- **Test:** a test channel with a hand-written `ChannelConfig` compiles. No behaviour
  yet.

### 10. Method handlers on `jsonrpc-rs`

- Generic dispatch functions `get_info::<C>`, `set_config::<C>`,
  `send_notification::<C>` registered manually as `JsonRpcMethod`.
- `SetConfig`: deserialize, `validate`, swap the `Arc`; the old config stays on error.
- `SendNotification`: reject unsupported `version` with
  `unsupported request version {v}, supported: [1]`; fail with `-32000` before the
  first `SetConfig`.
- `fn server<C: Channel>(channel: C) -> jsonrpc_rs::server::Server` (crate-private),
  using `map_requests(&tokio_parallel)`.
- **Test (integration, `tests/methods.rs`):** drive `Server::serve` with
  `stream::iter` and a `Vec` sink, as `jsonrpc/tests/spec.rs` does:
  - `GetInfo` returns the expected object.
  - `SendNotification` before `SetConfig` fails; after it succeeds.
  - A second `SetConfig` replaces the config; an invalid one keeps the old config.
  - `version: 2` is rejected.
  - A panicking `send_notification` yields `-32603`.

### 11. Shared writer

- Private `Outgoing { Response, Log }` and a writer task that drains an unbounded
  mpsc receiver into `FramedWrite<W, LinesCodec>`.
- The server sink is `tx.clone().with(...)`.
- **Test (integration):** responses and injected `Log` items arrive as complete lines
  on an in-memory writer (`tokio::io::duplex`), never interleaved.
- **Test:** closing the server's sink handle does not stop a second sender from
  delivering (verifies the open question about `mpsc::Sender::poll_close`).

### 12. `channel::Log` tracing layer

- `ChannelLogLayer` implementing `tracing_subscriber::Layer`: maps `TRACE`/`DEBUG` to
  `debug`, `INFO`, `WARN`, `ERROR`; the `message` field to `message`; other fields to
  the flat `fields` array; sends with `unbounded_send`.
- **Test (unit):** with a local subscriber (`tracing::subscriber::with_default`),
  `tracing::info!(id = 5, "hi")` produces
  `{"jsonrpc":"2.0","method":"channel::Log","params":{"level":"info","message":"hi","fields":["id",5]}}`
  without an `id`.

### 13. `run_with` over any reader/writer

- `run_with(channel, read, write, RunOptions)`: wires steps 10–12 together and
  stops when `read` reaches EOF.
- **Test (integration):** a full session over `tokio::io::duplex`: `GetInfo`,
  `SetConfig`, `SendNotification` and one log line, then EOF; `run_with` returns `Ok`.

### 14. Shutdown and `run`

- A shutdown future (SIGINT and, on Unix, SIGTERM) applied with `take_until`; drain
  bounded by `drain_timeout`.
- `run(channel)` and `run_with_subscriber(channel, |layer| subscriber)` on
  stdin/stdout.
- Make the shutdown future injectable inside `run_with` so it can be tested without
  real signals.
- **Test (integration):** triggering the injected shutdown while a slow request runs
  still delivers its response; a request that never finishes is abandoned after
  `drain_timeout` (use `tokio::time::pause`).
- **Test (manual, documented):** start an example channel, send SIGTERM, check the
  exit. Real signals are not exercised in CI.

### 15. Example channel

- `examples/echo.rs`: hand-written `ChannelConfig`, logs and returns a text detail.
- **Test (integration):** spawn the example binary, pipe a session into stdin,
  compare stdout lines.

## Phase 3: Derive macros

### 16. Macro crate skeleton and `ConfigEnum`

- `icinga-notifications-channel-macros` with `syn`, `quote`, `darling`.
- `#[derive(ConfigEnum)]` on unit enums: `fn variants() -> &'static [&'static str]`
  using serde names (`rename`, `rename_all`).
- Re-export behind the `derive` feature.
- **Test (unit):** variant names with and without `rename_all`.
- **Test (compile-fail, `trybuild`):** non-unit variants are rejected.

### 17. `ChannelConfig` derive without translations

- Field to `ConfigType` mapping (`String`, numbers, `bool`, `Secret`, `ConfigEnum`,
  `Vec`/`BTreeSet` of `ConfigEnum`, `Option<T>`), `#[config(text)]`,
  `#[config(min, max)]`.
- `required` unless `Option` or `#[serde(default)]`; defaults from
  `Default::default()` or the serde default function; names from serde renames.
- Temporarily use the field name as the `en_US` label so this step is testable alone.
- **Test (unit):** a struct covering every mapping produces the expected
  `config_attrs` JSON.
- **Test (compile-fail):** `min` on a `String`, `#[serde(flatten)]`, generic struct,
  unsupported field type; each error points at the right span.

### 18. Children

- `#[config(parent = "field", when = [...])]`, checked against the parent's
  `ConfigEnum` variants or `true`/`false`; a child cannot be a parent.
- **Test (unit):** the child appears under its parent with `parent_values`.
- **Test (compile-fail):** unknown parent, invalid `when` value, nested child.

### 19. Locale files

- `#[config(locales = "dir")]` relative to `CARGO_MANIFEST_DIR`; parse every
  `*.toml`, file stem is the locale; embed with `include_str!` so edits trigger
  rebuilds.
- Validate: `en_US.toml` exists and has a `label` for each field; unknown sections
  and option keys are errors naming the file, the key and the valid keys.
- `options` labels fall back to the variant name; missing `help` is omitted.
- Remove the temporary label from step 17.
- **Test (unit):** labels and help for two locales end up in `config_attrs`.
- **Test (compile-fail):** missing `en_US.toml`, missing `label`, unknown field,
  unknown option key, invalid locale file name.
- **Test:** touching a locale file rebuilds (checked manually once, documented).

### 20. `assert_complete_locale`

- `assert_complete_locale::<C>("de_DE")` for use in `cargo test`, listing every
  field and option missing a label or help.
- **Test (unit):** passes for a complete locale, fails with the full list otherwise.

### 21. Example and docs

- Switch `examples/echo.rs` to the derive with a `locales/` directory.
- README: quick start, translation file format, logging, shutdown behaviour,
  "never write to stdout".
- Doc comments on every public item, with doc-tests where possible.
- **Test:** doc-tests; the step-15 integration test still passes unchanged.

## Final validation

Run the example channel against a real Icinga Notifications instance:
`GetInfo` shows the translated form in Icinga Notifications Web, `SetConfig` applies
changes without a restart, and a notification is delivered with its log lines
visible in the Icinga Notifications log.
