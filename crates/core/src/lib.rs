//! Wuxia Local core.
//!
//! Everything here is pure logic with no OS or network access, so it can be
//! unit-tested anywhere. The Windows application (`wuxia-app`) wires these
//! pieces to screen capture, OCR, llama-server and the overlay.

pub mod cache;
pub mod detect;
pub mod glossary;
pub mod history;
pub mod image;
pub mod prompt;
pub mod similarity;
pub mod text;
pub mod tsv;
