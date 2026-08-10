//! Ce que la carte accepte vraiment.
//!
//! `cargo run --release -p sculpt-app --example gpu_limits`
//!
//! Les limites par défaut de wgpu sont celles qu'un navigateur garantit, pas
//! celles d'une carte de bureau: un tampon y est plafonné à 256 Mo, ce qu'un
//! maillage dépasse vers 3,7 millions de sommets. Demander plus que la limite
//! est une erreur de validation, et sur ce chemin la fenêtre se ferme. Cet
//! exemple imprime les deux séries de chiffres côte à côte.

#[path = "../src/gpu_vertex.rs"]
mod gpu_vertex;

fn main() {
    let instance = wgpu::Instance::new(
        wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
    );
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("aucune carte utilisable");

    let info = adapter.get_info();
    println!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);
    println!();

    let mine = adapter.limits();
    let portable = wgpu::Limits::default();
    let mo = |n: u64| n as f64 / 1.0e6;
    // Un sommet occupe ses deux flux, dont le plus gros est le chaud: c'est
    // lui qui décide de la taille du tampon qui plafonne en premier.
    let sommets = |n: u64| n / gpu_vertex::HOT_BYTES as u64;

    println!("{:<34} {:>14} {:>14}", "", "par défaut", "cette carte");
    println!(
        "{:<34} {:>11.0} Mo {:>11.0} Mo",
        "tampon le plus grand",
        mo(portable.max_buffer_size),
        mo(mine.max_buffer_size)
    );
    println!(
        "{:<34} {:>11.0} Mo {:>11.0} Mo",
        "liaison de stockage la plus grande",
        mo(portable.max_storage_buffer_binding_size as u64),
        mo(mine.max_storage_buffer_binding_size as u64)
    );
    println!();
    println!(
        "sommets tenables dans un tampon: {:.1} M par défaut, {:.1} M ici",
        sommets(portable.max_buffer_size) as f64 / 1.0e6,
        sommets(mine.max_buffer_size) as f64 / 1.0e6
    );
    println!(
        "en gardant la marge de croissance des tampons: {:.1} M par défaut, {:.1} M ici",
        sommets(portable.max_buffer_size / 3) as f64 / 1.0e6,
        sommets(mine.max_buffer_size / 3) as f64 / 1.0e6
    );
}
