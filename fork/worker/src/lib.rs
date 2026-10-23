// The fork's home AI worker: a headless, read-only Ente client that reads
// the fileKeys clients seal to it, runs CPU analysis stages and merges the
// results into each file's `mldata` under `x_enrich`.
// See fork/worker/README.md and fork/docs/worker-api.md.

pub mod api;
pub mod config;
pub mod harden;
pub mod identity;
pub mod mldata;
pub mod pipeline;
pub mod secure_mem;
pub mod stages;
