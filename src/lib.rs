//! Glide: a fast, native macOS screen recorder and editor.
//!
//! Behaviour was reverse engineered from Screen Studio with REA (see
//! `analysis/SCREEN_STUDIO.md`); all code here is written independently.
//! The capture pipeline builds on rec-rs by Michael Shimeles.

pub mod engine;
pub mod project;
pub mod render;
pub mod synth;
pub mod capture;
pub mod util;
pub mod diag;
pub mod hw;
pub mod media;
pub mod export;
pub mod recorder;
pub mod transcribe;
pub mod exposure;
pub mod audiocheck;
pub mod whisper;
pub mod app;
