//! The PC's video card and memory, and which translation model suits it
//! (`wuxia_core::hwfit`). Read once at start-up through DXGI — the same list of
//! video cards games see; nothing is installed or probed.

use crate::config::{Config, ModelPreset};
use wuxia_core::hwfit::{self, Candidate, Fit};

#[derive(Debug, Clone, Default)]
pub struct Hardware {
    /// The video card with the most own memory.
    pub gpu_name: Option<String>,
    /// Its own (dedicated) memory, GB. None: no separate video card
    /// (built-in graphics borrow system memory and are too slow for the model).
    pub vram_gb: Option<f32>,
    pub ram_gb: f32,
}

impl Hardware {
    /// "NVIDIA GeForce RTX 3060 (12 ГБ) · ОЗУ 32 ГБ"
    pub fn describe(&self) -> String {
        let gpu = match (&self.gpu_name, self.vram_gb) {
            (Some(n), Some(v)) => format!("{n} ({})", gb(v)),
            (Some(n), None) => format!("{n} (встроенная)"),
            _ => "видеокарта не найдена".into(),
        };
        format!("{gpu} · ОЗУ {}", gb(self.ram_gb))
    }
}

fn gb(v: f32) -> String {
    if v >= 10.0 || (v - v.round()).abs() < 0.05 {
        format!("{:.0} ГБ", v.round())
    } else {
        format!("{v:.1} ГБ").replace('.', ",")
    }
}

#[cfg(windows)]
pub fn detect() -> Hardware {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut hw = Hardware::default();
    unsafe {
        let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if GlobalMemoryStatusEx(&mut m).is_ok() {
            hw.ram_gb = m.ullTotalPhys as f32 / 1_073_741_824.0;
        }
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return hw };
        let mut best: Option<(String, u64)> = None;
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(d) = adapter.GetDesc1() else { continue };
            // Software renderer ("Microsoft Basic Render Driver").
            if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == 0x1414 {
                continue;
            }
            let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').trim().to_string();
            let vram = d.DedicatedVideoMemory as u64;
            if best.as_ref().map_or(true, |(_, v)| vram > *v) {
                best = Some((name, vram));
            }
        }
        if let Some((name, vram)) = best {
            let v = vram as f32 / 1_073_741_824.0;
            // Built-in graphics report a few hundred MB of their own.
            hw.vram_gb = (v >= 1.5).then_some(v);
            hw.gpu_name = Some(name);
        }
    }
    hw
}

#[cfg(not(windows))]
pub fn detect() -> Hardware {
    Hardware::default()
}

/// Models worth recommending, best quality first (Hy-MT2: the best translators in our tests).
fn candidates(cfg: &Config) -> Vec<Candidate<'_>> {
    let mut v: Vec<Candidate> = ["hymt2-7b", "hymt2-1.8b"]
        .iter()
        .filter_map(|id| cfg.models.iter().find(|m| m.id == *id))
        .map(|m| Candidate { id: m.id.as_str(), size_gb: m.size_gb })
        .collect();
    if v.is_empty() {
        v = cfg.models.iter().map(|m| Candidate { id: m.id.as_str(), size_gb: m.size_gb }).collect();
    }
    v
}

/// The model for this PC and whether it goes on the video card.
pub fn recommend(cfg: &Config, hw: &Hardware) -> (String, bool) {
    let (id, gpu) = hwfit::recommend(&candidates(cfg), cfg.llm.ctx_size, hw.vram_gb, hw.ram_gb);
    (id.to_string(), gpu)
}

/// Size of a model for the fit check: the actual file if it is downloaded.
fn size_of(m: &ModelPreset) -> f32 {
    std::fs::metadata(&m.path).map(|md| md.len() as f32 / 1_073_741_824.0).ok().filter(|s| *s > 0.1).unwrap_or(m.size_gb)
}

pub fn fit(cfg: &Config, m: &ModelPreset, hw: &Hardware) -> Fit {
    if hw.ram_gb <= 0.0 {
        return Fit::Gpu; // not detected: do not warn
    }
    hwfit::fit(size_of(m), cfg.llm.ctx_size, hw.vram_gb, hw.ram_gb)
}

/// A warning for the model card, if this model may not run well here.
pub fn warning(cfg: &Config, m: &ModelPreset, hw: &Hardware) -> Option<String> {
    let need = hwfit::need_gb(size_of(m), cfg.llm.ctx_size);
    let (rec, _) = recommend(cfg, hw);
    let rec_name = cfg.models.iter().find(|x| x.id == rec).map(|x| short_name(&x.name)).unwrap_or_default();
    let better = if rec != m.id && !rec_name.is_empty() { format!(" Подойдёт: {rec_name}.") } else { String::new() };
    match fit(cfg, m, hw) {
        Fit::Gpu => None,
        Fit::Tight if cfg.llm.gpu_layers > 0 => Some(format!(
            "Может не поместиться: модели нужно около {} видеопамяти, у видеокарты {}, и около {} занимает игра. Возможны тормоза или ошибка запуска.{better}",
            gb(need),
            gb(hw.vram_gb.unwrap_or(0.0)),
            gb(hwfit::GAME_VRAM_GB)
        )),
        Fit::Tight | Fit::Cpu if cfg.llm.gpu_layers > 0 && hw.vram_gb.is_some() => Some(format!(
            "Не поместится в видеопамять ({}): нужно около {}. Включите работу на процессоре (будет медленнее) или выберите модель меньше.{better}",
            gb(hw.vram_gb.unwrap_or(0.0)),
            gb(need)
        )),
        Fit::Tight | Fit::Cpu => None,
        Fit::TooBig => Some(format!("Не хватит памяти компьютера: модели нужно около {}, а свободно меньше.{better}", gb(need))),
    }
}

/// "Hy-MT2 7B Q4_K_M — качественный (мощный ПК)" → "Hy-MT2 7B".
pub fn short_name(name: &str) -> String {
    let base = name.split('—').next().unwrap_or(name).trim();
    // Drop the quantisation tag ("Q4_K_M"), keep "Qwen3.5".
    base.split_whitespace().filter(|w| !(w.starts_with('Q') && w.chars().nth(1).map_or(false, |c| c.is_ascii_digit()))).collect::<Vec<_>>().join(" ")
}
