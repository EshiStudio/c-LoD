//! Block palette: material name -> map color (the classic Minecraft map
//! palette), plus color -> representative block for impostor rendering.
//!
//! The table covers the surface materials that matter for distant terrain.
//! Unknown materials fall back to a deterministic name-hash grey, and prefix
//! heuristics (`*_leaves`, `*_log`, `*_planks`, ...) cover modded/vanilla
//! families without listing every block.

pub const FALLBACK: [u8; 3] = [120, 120, 120];

/// Curated map colors (vanilla MaterialColor RGB values).
pub fn map_color(material: &str) -> [u8; 3] {
    if let Some(c) = exact().get(material) {
        return *c;
    }
    for (suffix, color) in SUFFIXES {
        if material.ends_with(suffix) {
            return *color;
        }
    }
    fallback_for(material)
}

/// Deterministic grey for unknown materials (stable across runs).
fn fallback_for(material: &str) -> [u8; 3] {
    let mut h: u32 = 2166136261;
    for b in material.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let v = 100 + (h % 56) as u8; // 100..155 grey
    [v, v, v]
}

/// Pick a representative block whose map color is closest to `rgb`.
/// Water-ish colors map to water, greens to grass, greys to stone, ...
pub fn representative_block(rgb: [u8; 3]) -> &'static str {
    let mut best = "stone";
    let mut best_d = i64::MAX;
    for (name, color) in REPRESENTATIVES {
        let d = dist(*color, rgb);
        if d < best_d {
            best_d = d;
            best = name;
        }
    }
    best
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i64 {
    let dr = a[0] as i64 - b[0] as i64;
    let dg = a[1] as i64 - b[1] as i64;
    let db = a[2] as i64 - b[2] as i64;
    dr * dr + dg * dg + db * db
}

use std::collections::HashMap;
use std::sync::OnceLock;

fn exact() -> &'static HashMap<&'static str, [u8; 3]> {
    static T: OnceLock<HashMap<&'static str, [u8; 3]>> = OnceLock::new();
    T.get_or_init(|| EXACT_LIST.iter().copied().collect())
}

const EXACT_LIST: &[(&str, [u8; 3])] = &[
    // greens / foliage
    ("grass_block", [127, 178, 56]),
    ("moss_block", [110, 143, 59]),
    ("mossy_cobblestone", [110, 143, 59]),
    ("short_grass", [127, 178, 56]),
    ("tall_grass", [127, 178, 56]),
    ("fern", [127, 178, 56]),
    ("large_fern", [127, 178, 56]),
    ("azalea", [127, 178, 56]),
    // dirt family
    ("dirt", [151, 109, 77]),
    ("coarse_dirt", [114, 84, 48]),
    ("rooted_dirt", [151, 109, 77]),
    ("grass_path", [151, 109, 77]),
    ("dirt_path", [151, 109, 77]),
    ("podzol", [91, 67, 35]),
    ("mycelium", [112, 101, 89]),
    ("mud", [60, 52, 44]),
    ("packed_mud", [97, 72, 50]),
    // stone family
    ("stone", [125, 125, 125]),
    ("cobblestone", [127, 127, 127]),
    ("andesite", [136, 136, 136]),
    ("diorite", [188, 188, 188]),
    ("granite", [149, 103, 84]),
    ("deepslate", [80, 80, 84]),
    ("cobbled_deepslate", [80, 80, 84]),
    ("tuff", [108, 109, 102]),
    ("basalt", [65, 65, 65]),
    ("blackstone", [37, 29, 37]),
    ("bedrock", [84, 84, 84]),
    ("obsidian", [21, 18, 29]),
    // sand / gravel
    ("sand", [250, 228, 157]),
    ("red_sand", [151, 90, 26]),
    ("sandstone", [250, 228, 157]),
    ("gravel", [136, 126, 126]),
    // water / ice
    ("water", [63, 118, 228]),
    ("ice", [160, 166, 255]),
    ("packed_ice", [145, 151, 255]),
    ("blue_ice", [116, 167, 253]),
    ("frosted_ice", [160, 166, 255]),
    // snow
    ("snow", [255, 255, 255]),
    ("snow_block", [255, 255, 255]),
    ("powder_snow", [255, 255, 255]),
    // wood
    ("oak_log", [109, 84, 51]),
    ("spruce_log", [60, 45, 33]),
    ("birch_log", [216, 206, 188]),
    ("jungle_log", [91, 84, 45]),
    ("acacia_log", [107, 111, 63]),
    ("dark_oak_log", [59, 47, 27]),
    ("mangrove_log", [84, 50, 41]),
    ("cherry_log", [101, 67, 74]),
    ("bamboo_block", [133, 169, 77]),
    // nether
    ("netherrack", [78, 17, 17]),
    ("soul_sand", [60, 44, 34]),
    ("soul_soil", [46, 33, 27]),
    ("nether_bricks", [44, 22, 26]),
    ("crimson_nylium", [133, 34, 33]),
    ("warped_wart_block", [22, 125, 117]),
    // end
    ("end_stone", [218, 224, 162]),
    ("purpur_block", [167, 107, 167]),
    // clay / terracotta
    ("clay", [160, 166, 179]),
    ("terracotta", [152, 93, 94]),
    ("white_terracotta", [210, 178, 161]),
    ("orange_terracotta", [162, 84, 38]),
    ("yellow_terracotta", [186, 133, 35]),
    ("red_terracotta", [143, 61, 47]),
    ("brown_terracotta", [77, 51, 35]),
    ("light_gray_terracotta", [135, 106, 97]),
    // ore-ish surface markers
    ("coal_ore", [125, 125, 125]),
    ("iron_ore", [175, 142, 116]),
    ("copper_ore", [187, 133, 106]),
    ("gold_ore", [156, 127, 78]),
    ("redstone_ore", [142, 84, 49]),
    ("lapis_ore", [109, 125, 140]),
    ("diamond_ore", [119, 154, 156]),
    ("emerald_ore", [115, 150, 112]),
    // misc surface
    ("calcite", [223, 224, 220]),
    ("dripstone_block", [134, 107, 92]),
    ("smooth_basalt", [72, 72, 78]),
    ("sculk", [12, 29, 36]),
    ("pumpkin", [198, 119, 7]),
    ("melon", [111, 144, 31]),
    ("hay_block", [168, 132, 47]),
    ("oak_leaves", [127, 178, 56]),
    ("spruce_leaves", [63, 118, 57]),
    ("birch_leaves", [128, 151, 83]),
    ("jungle_leaves", [73, 131, 10]),
    ("acacia_leaves", [110, 143, 59]),
    ("dark_oak_leaves", [63, 91, 29]),
    ("mangrove_leaves", [51, 118, 25]),
    ("cherry_leaves", [233, 174, 204]),
    ("azalea_leaves", [97, 153, 52]),
    ("pale_oak_leaves", [141, 162, 132]),
    ("pale_moss_block", [141, 162, 132]),
    ("pale_oak_log", [85, 78, 67]),
    ("resin_block", [228, 108, 10]),
];

/// `ends_with` heuristic families (checked in order).
const SUFFIXES: &[(&str, [u8; 3])] = &[
    ("_leaves", [110, 143, 59]),
    ("_log", [109, 84, 51]),
    ("_wood", [109, 84, 51]),
    ("_planks", [162, 130, 78]),
    ("_wool", [232, 232, 232]),
    ("_carpet", [232, 232, 232]),
    ("_terracotta", [152, 93, 94]),
    ("_concrete_powder", [125, 125, 125]),
    ("_sand", [250, 228, 157]),
    ("_nylium", [78, 17, 17]),
    ("_ice", [160, 166, 255]),
    ("_corridor", [125, 125, 125]),
    ("_glass", [255, 255, 255]),
];

/// Canonical impostor blocks by color family (name -> map color).
const REPRESENTATIVES: &[(&str, [u8; 3])] = &[
    ("grass_block", [127, 178, 56]),
    ("water", [63, 118, 228]),
    ("dirt", [151, 109, 77]),
    ("stone", [125, 125, 125]),
    ("deepslate", [80, 80, 84]),
    ("sand", [250, 228, 157]),
    ("red_sand", [151, 90, 26]),
    ("gravel", [136, 126, 126]),
    ("snow_block", [255, 255, 255]),
    ("ice", [160, 166, 255]),
    ("podzol", [91, 67, 35]),
    ("mud", [60, 52, 44]),
    ("oak_log", [109, 84, 51]),
    ("spruce_log", [60, 45, 33]),
    ("birch_log", [216, 206, 188]),
    ("netherrack", [78, 17, 17]),
    ("soul_sand", [60, 44, 34]),
    ("end_stone", [218, 224, 162]),
    ("terracotta", [152, 93, 94]),
    ("moss_block", [110, 143, 59]),
    ("mycelium", [112, 101, 89]),
    ("obsidian", [21, 18, 29]),
    ("clay", [160, 166, 179]),
    ("granite", [149, 103, 84]),
    ("cherry_leaves", [233, 174, 204]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn exact_entries_unique() {
        let mut seen = HashSet::new();
        for (name, _) in EXACT_LIST {
            assert!(seen.insert(*name), "duplicate palette entry: {}", name);
        }
        let mut seen = HashSet::new();
        for (name, _) in REPRESENTATIVES {
            assert!(seen.insert(*name), "duplicate representative: {}", name);
        }
    }

    #[test]
    fn rgb_values_in_range() {
        for (name, [r, g, b]) in EXACT_LIST {
            assert!([r, g, b].iter().all(|v| (0..=255).contains(*v)), "{}", name);
        }
    }

    #[test]
    fn known_colors() {
        assert_eq!(map_color("grass_block"), [127, 178, 56]);
        assert_eq!(map_color("water"), [63, 118, 228]);
        assert_eq!(map_color("oak_leaves"), [127, 178, 56]);
        assert_eq!(map_color("deepslate"), [80, 80, 84]);
    }

    #[test]
    fn suffix_fallbacks() {
        assert_eq!(map_color("amazing_mod_leaves"), [110, 143, 59]);
        assert_eq!(map_color("rubber_log"), [109, 84, 51]);
        assert_eq!(map_color("blue_wool"), [232, 232, 232]);
    }

    #[test]
    fn unknown_material_is_deterministic_grey() {
        let a = map_color("totally_unknown_block");
        let b = map_color("totally_unknown_block");
        assert_eq!(a, b);
        assert!(a[0] >= 100 && a[0] <= 155);
        assert_eq!(a[0], a[1]);
        assert_eq!(a[1], a[2]);
    }

    #[test]
    fn representative_picks_family() {
        // grass color -> grass block
        assert_eq!(representative_block([127, 178, 56]), "grass_block");
        // water color -> water
        assert_eq!(representative_block([63, 118, 228]), "water");
        // pure white -> snow
        assert_eq!(representative_block([255, 255, 255]), "snow_block");
    }

    #[test]
    fn representative_names_are_lowercase_materials() {
        for (name, _) in REPRESENTATIVES {
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }
}
