//! Named PBR paint materials, independent of the viewport lighting.
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    pub color: [f32; 3],
    pub roughness: f32,
    pub metalness: f32,
}

pub fn defaults() -> Vec<Material> {
    [
        ("Terracotta", [0.68, 0.30, 0.19], 0.85, 0.0),
        ("Porcelain", [0.91, 0.90, 0.85], 0.22, 0.0),
        ("Graphite", [0.09, 0.10, 0.12], 0.50, 0.0),
        ("Jade", [0.16, 0.47, 0.31], 0.27, 0.0),
        ("Bronze", [0.62, 0.36, 0.15], 0.32, 1.0),
        ("Silver", [0.83, 0.85, 0.88], 0.20, 1.0),
    ]
    .into_iter()
    .map(|(name, color, roughness, metalness)| Material {
        name: name.into(),
        color,
        roughness,
        metalness,
    })
    .collect()
}
