# connector-runtime documentation

The runtime is the sim-neutral part of an official Stintmate connector. Start with the
[README](../README.md) for what it is and how to depend on it, then:

| Document | Read it to |
|---|---|
| [architecture.md](architecture.md) | see how the three processes of a connector work: threads, links and backoff, the outbox, the sink and session resume, the status, the texts, the development tag, and what the connector supplies |
| [embedding.md](embedding.md) | write a connector on the runtime: the `ConnectorSpec` field by field, the three `main` functions, the `build.rs`, the UI catalog and its check, `ResumeProbe`, `WriteSink`, and the tests to copy |

The protocols themselves are not described here. They belong to the
[connector kit](https://github.com/Stintmate-App/connector-kit) (the connector protocol, the
contract, the write API, the descriptor, the catalogs format) and to
[Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc) (the transport). When this
documentation says "contract.md" or "catalogs.md" it means those files of the connector kit.

The code is the reference: every crate has `#![deny(missing_docs)]`, and `cargo doc --workspace
--no-deps --open` shows the public API of all of them.
