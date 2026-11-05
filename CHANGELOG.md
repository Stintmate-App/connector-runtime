# Changelog

All notable changes to this project are written here.

## Unreleased

- Repository created.
- Cargo workspace with the toolchain pin, the static C runtime setting, `deny.toml`, `NOTICE`,
  `SECURITY.md` and a manually started CI workflow.
- `stintmate-connector-resources`: the connector icons drawn in code (`Glyph::A`, `Glyph::I`,
  `level_rgb`, `render_rgba`, `ico_bytes`) and the Windows resource script (`version_rc`).
- `stintmate-connector-platform`: the workspace resolver, logging, single-instance mutex and stop
  event, process, window, game and timer helpers, and `ConnectorSpec` / `GameSpec` with their
  validation. The development tag is read from `STINTMATE_DEV_INSTANCE`, then from the spec's
  aliases; tagged mutex names append the tag (`Local\<prefix>.UI.<tag>`).
- `stintmate-connector-i18n`: the text catalogs contract loader (`load` with the catalog id,
  `Catalog`, plural rules, validation, pseudo-locale) and `Embedded`, which holds a connector's
  embedded English catalog after checking its schema and catalog id. The contract's vectors are
  tested against it.
- `stintmate-connector-wire`: the write API message structs (`LapChannels` takes owned or static
  channel names), the 32-byte `connector.telemetry` sample with its flags, the announcement
  catalog format (`AnnounceCatalog`, `validate_catalog`, canonical JSON, `sha12`, source hashes,
  typed params), the announcement rate window, `standard_actions`, and the test schema checker
  with the connector kit's write API and protocol schemas. Every message is validated against
  the write API schema in the tests.
