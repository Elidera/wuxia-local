//! Which translation model suits this PC: video memory, system memory, and
//! the game running at the same time.
//!
//! The model runs next to the game, which itself takes several gigabytes of
//! video memory, so a model is only put on the video card when it fits next
//! to the game; otherwise it runs on the processor (slower, but it works).

/// Video memory a modern game keeps for itself, GB.
pub const GAME_VRAM_GB: f32 = 3.0;

/// Video (or system) memory a model needs: the weights plus the context and
/// working buffers of llama.cpp.
pub fn need_gb(size_gb: f32, ctx_size: u32) -> f32 {
    size_gb * 1.1 + 0.35 + 0.3 * (ctx_size as f32 / 4096.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// Fits on the video card next to the game.
    Gpu,
    /// Fits the card, but not together with the game: may stutter or fail.
    Tight,
    /// No suitable video card: runs on the processor.
    Cpu,
    /// Not enough memory at all.
    TooBig,
}

/// How a model of `size_gb` fits a PC with `vram_gb` of video memory (None — no
/// discrete card) and `ram_gb` of system memory.
pub fn fit(size_gb: f32, ctx_size: u32, vram_gb: Option<f32>, ram_gb: f32) -> Fit {
    let need = need_gb(size_gb, ctx_size);
    match vram_gb {
        Some(v) if v - GAME_VRAM_GB >= need => Fit::Gpu,
        Some(v) if v >= need => Fit::Tight,
        _ if ram_gb - 6.0 >= need => Fit::Cpu, // the game and Windows take ~6 GB of RAM
        _ => Fit::TooBig,
    }
}

/// A model the program offers: id, download size.
pub struct Candidate<'a> {
    pub id: &'a str,
    pub size_gb: f32,
}

/// The best model for this PC from `candidates` (best quality first) and
/// whether to use the video card. Falls back to the last (smallest) one on the
/// processor.
pub fn recommend<'a>(candidates: &[Candidate<'a>], ctx_size: u32, vram_gb: Option<f32>, ram_gb: f32) -> (&'a str, bool) {
    for c in candidates {
        if fit(c.size_gb, ctx_size, vram_gb, ram_gb) == Fit::Gpu {
            return (c.id, true);
        }
    }
    (candidates.last().map_or("", |c| c.id), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODELS: [Candidate; 2] = [Candidate { id: "hymt2-7b", size_gb: 4.62 }, Candidate { id: "hymt2-1.8b", size_gb: 1.13 }];

    #[test]
    fn picks_by_video_memory() {
        assert_eq!(recommend(&MODELS, 4096, Some(12.0), 32.0), ("hymt2-7b", true));
        assert_eq!(recommend(&MODELS, 4096, Some(8.0), 16.0), ("hymt2-1.8b", true));
        assert_eq!(recommend(&MODELS, 4096, Some(6.0), 16.0), ("hymt2-1.8b", true));
        // Small card: the game needs it all.
        assert_eq!(recommend(&MODELS, 4096, Some(4.0), 16.0), ("hymt2-1.8b", false));
        assert_eq!(recommend(&MODELS, 4096, None, 8.0), ("hymt2-1.8b", false));
    }

    #[test]
    fn warns_when_tight() {
        assert_eq!(fit(4.62, 4096, Some(8.0), 16.0), Fit::Tight);
        assert_eq!(fit(4.62, 4096, Some(4.0), 16.0), Fit::Cpu);
        assert_eq!(fit(4.62, 4096, None, 8.0), Fit::TooBig);
        assert_eq!(fit(1.13, 4096, Some(6.0), 16.0), Fit::Gpu);
        assert!((need_gb(1.13, 4096) - 1.89).abs() < 0.05);
    }
}
