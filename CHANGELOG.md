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
