//! Deterministic avatar generator.
//!
//! Generates unique braille-pixel art glyphs from a seed string (agent name).
//! Uses radial symmetry with hash-derived pattern data.

use sha2::{Digest, Sha256};

/// Grid dimensions in pixels. Avatars are 16x16 pixels = 8x4 braille chars.
pub const AVATAR_W: usize = 16;
pub const AVATAR_H: usize = 16;

/// Unicode braille base codepoint.
const BRAILLE_BASE: u32 = 0x2800;

/// Dot bit masks per position: `[row][col]` within a 2x4 braille cell.
const BRAILLE_DOTS: [[u32; 2]; 4] = [
    [1, 8],    // row 0
    [2, 16],   // row 1
    [4, 32],   // row 2
    [64, 128], // row 3
];

/// A generated avatar: braille character grid ready for display.
#[derive(Debug, Clone)]
pub struct Avatar {
    /// Braille character rows (each row = `AVATAR_W / 2` characters).
    pub lines: Vec<String>,
}

impl Avatar {
    /// Width in terminal columns (braille chars).
    pub fn char_width(&self) -> usize {
        AVATAR_W / 2
    }

    /// Height in terminal rows.
    pub fn char_height(&self) -> usize {
        self.lines.len()
    }
}

/// Generate an avatar from a seed string.
///
/// The same seed always produces the same avatar. Agent names work well as seeds.
/// Optional `extra_entropy` (e.g. SSH key fingerprint, profile hash) adds more
/// uniqueness without breaking determinism.
pub fn generate(seed: &str, extra_entropy: Option<&str>) -> Avatar {
    let hash = hash_seed(seed, extra_entropy);
    let grid = radial_symmetry(&hash, AVATAR_W, AVATAR_H, 4);
    let lines = grid_to_braille(&grid, AVATAR_W, AVATAR_H);
    Avatar { lines }
}

/// Hash a seed string into 32 bytes via SHA-256.
fn hash_seed(seed: &str, extra: Option<&str>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    if let Some(extra) = extra {
        hasher.update(extra.as_bytes());
    }
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Generate a radially symmetric boolean grid from hash data.
///
/// The grid is divided into `slices` angular wedges. Pattern data is derived
/// from the hash bytes, then mirrored radially so the avatar looks like a
/// sigil, glyph, or mandala.
fn radial_symmetry(hash: &[u8; 32], w: usize, h: usize, slices: usize) -> Vec<Vec<bool>> {
    // Use the first 4 bytes as a phase so each seed rotates the radial layout.
    let phase = u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]]) as f64 / (u32::MAX as f64)
        * std::f64::consts::TAU;

    // Expand remaining bytes into deterministic on/off pattern bits.
    let pattern: Vec<bool> = hash[4..]
        .iter()
        .flat_map(|&byte| (0..8).map(move |bit| byte & (1 << bit) != 0))
        .collect();

    let cx = w as f64 / 2.0;
    let cy = h as f64 / 2.0;
    let max_r = (w.min(h) as f64) / 2.0;
    let pattern_len = pattern.len();

    let mut grid = vec![vec![false; w]; h];

    for y in 0..h {
        for x in 0..w {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let r = (dx * dx + dy * dy).sqrt() / max_r;

            if r > 1.0 {
                continue;
            }

            let angle = (dy.atan2(dx) + phase).rem_euclid(std::f64::consts::TAU);
            let slice_idx = (angle / std::f64::consts::TAU * slices as f64) as usize % slices;
            let band = (r * 8.0) as usize;
            let idx = (band * slices + slice_idx) % pattern_len;
            grid[y][x] = pattern[idx];
        }
    }

    grid
}

/// Convert a pixel grid to Unicode braille characters.
fn grid_to_braille(grid: &[Vec<bool>], w: usize, h: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(h / 4);

    for row in (0..h).step_by(4) {
        let mut line = String::with_capacity(w / 2);

        for col in (0..w).step_by(2) {
            let mut code = BRAILLE_BASE;

            for dy in 0..4 {
                for dx in 0..2 {
                    let y = row + dy;
                    let x = col + dx;
                    if y < h && x < w && grid[y][x] {
                        code |= BRAILLE_DOTS[dy][dx];
                    }
                }
            }

            // SAFETY: all generated codepoints are valid Unicode braille (U+2800..U+28FF)
            line.push(char::from_u32(code).unwrap_or('\u{2800}'));
        }

        lines.push(line);
    }

    lines
}

/// Render an avatar as a bordered box (for CLI/test output).
pub fn render_boxed(avatar: &Avatar, label: &str) -> String {
    let w = avatar.char_width();
    let border: String = "─".repeat(w + 2);
    let mut out = String::new();

    if !label.is_empty() {
        out.push_str(label);
        out.push('\n');
    }

    out.push('┌');
    out.push_str(&border);
    out.push('┐');
    out.push('\n');

    for line in &avatar.lines {
        out.push('│');
        out.push(' ');
        out.push_str(line);
        out.push(' ');
        out.push('│');
        out.push('\n');
    }

    out.push('└');
    out.push_str(&border);
    out.push('┘');

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_is_deterministic() {
        let a1 = generate("hermes", None);
        let a2 = generate("hermes", None);
        assert_eq!(a1.lines, a2.lines);
    }

    #[test]
    fn different_seeds_differ() {
        let a1 = generate("hermes", None);
        let a2 = generate("codex", None);
        assert_ne!(a1.lines, a2.lines);
    }

    #[test]
    fn extra_entropy_changes_output() {
        let a1 = generate("hermes", None);
        let a2 = generate("hermes", Some("ssh-fingerprint-abc"));
        assert_ne!(a1.lines, a2.lines);
    }

    #[test]
    fn correct_dimensions() {
        let avatar = generate("test", None);
        assert_eq!(avatar.char_width(), AVATAR_W / 2);
        assert_eq!(avatar.char_height(), AVATAR_H / 4);
    }

    #[test]
    fn all_lines_same_length() {
        let avatar = generate("test-agent-long-name", None);
        let len = avatar.lines[0].len();
        for line in &avatar.lines {
            assert_eq!(line.len(), len, "all avatar lines must be same length");
        }
    }
}
