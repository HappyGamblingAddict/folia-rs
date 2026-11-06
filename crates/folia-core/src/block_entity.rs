//! Typed block-entity storage and tickers.
//!
//! Filled by the P6 parallel worker. The module owns chest/furnace/sign/spawner
//! state and its chunk NBT encoding; world storage only needs to carry the
//! serialized payload.
