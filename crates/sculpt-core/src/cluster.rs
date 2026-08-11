//! Partition du maillage en paquets de faces voisines.
//!
//! Le rendu envoie aujourd'hui tout le modèle à chaque image. À trente millions
//! de triangles, dessiner ce qu'on ne voit pas coûte plus cher que tout le
//! reste réuni. Un paquet est l'unité qui permet de décider: il porte sa boîte,
//! donc on peut l'écarter d'un test, et il occupe une plage contiguë du tampon
//! d'indices, donc on peut le dessiner seul.
//!
//! Les faces sont rangées par courbe de Morton sur leur centre. Deux faces
//! voisines dans l'espace se retrouvent voisines dans le tampon, ce qui donne
//! des paquets compacts sans construire d'arbre, et rend au passage le tampon
//! d'indices bien plus agréable au cache du GPU.
//!
//! Rien ici ne connaît le GPU: la partition est une propriété du maillage, et
//! l'application en fait ce qu'elle veut.

use crate::mesh::Mesh;
use glam::Vec3;
use rayon::prelude::*;

/// Nombre de faces visé par paquet.
///
/// Assez gros pour que le test de visibilité soit négligeable devant le dessin,
/// assez petit pour que rejeter un paquet rejette vraiment quelque chose. La
/// même fourchette que les meshlets des cartes récentes.
pub const TARGET_FACES: usize = 2048;

/// Trou maximal comblé entre deux plages plutôt que d'ouvrir un appel de plus.
const GAP_TOLERANCE: u32 = 2 * TARGET_FACES as u32;

/// Un paquet de faces contiguës dans le tampon d'indices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cluster {
    /// Première face du paquet, en indice de face.
    pub first: u32,
    /// Nombre de faces.
    pub count: u32,
    /// Coin inférieur de la boîte englobante.
    pub lo: Vec3,
    /// Coin supérieur.
    pub hi: Vec3,
    /// Direction commune des faces du paquet.
    ///
    /// Avec l'ouverture ci-dessous, elle décrit le cône qui contient toutes
    /// leurs normales. Sur une forme fermée, la moitié des paquets tourne le
    /// dos à la caméra à tout instant: c'est autant de dessiné pour rien, et
    /// c'est ce que la seule boîte englobante ne peut pas dire.
    pub axis: Vec3,
    /// Cosinus du demi-angle du cône. Négatif quand les normales sont trop
    /// dispersées pour qu'un cône veuille dire quelque chose.
    pub cos_spread: f32,
}

impl Cluster {
    pub fn center(&self) -> Vec3 {
        (self.lo + self.hi) * 0.5
    }

    /// Rayon de la sphère qui contient la boîte.
    pub fn radius(&self) -> f32 {
        (self.hi - self.lo).length() * 0.5
    }

    /// Vrai quand toutes les faces du paquet tournent le dos à l'oeil.
    ///
    /// Le test des meshlets: si l'angle entre l'axe du cône et la direction du
    /// regard dépasse l'ouverture du cône, plus la marge que prend le paquet
    /// vu de l'oeil, alors aucune de ses faces ne peut être de face.
    ///
    /// Conservateur par construction: un paquet dont les normales partent dans
    /// tous les sens n'est jamais écarté. Écarter à tort laisserait un trou
    /// dans le modèle, ce qui est bien pire que dessiner un peu trop.
    pub fn facing_away(&self, eye: Vec3) -> bool {
        if self.cos_spread <= 0.0 {
            return false;
        }
        let to_center = self.center() - eye;
        let distance = to_center.length();
        if distance <= self.radius() {
            return false; // l'oeil est dedans
        }
        let dir = to_center / distance;
        // La normale la plus tournée vers l'oeil fait l'ouverture du cône avec
        // son axe, donc tout le paquet est de dos quand l'axe et le regard sont
        // à moins d'un angle droit moins cette ouverture: en cosinus, cela se
        // compare au sinus de l'ouverture, pas à son cosinus.
        let sin_spread = (1.0 - self.cos_spread * self.cos_spread).max(0.0).sqrt();
        // Ce que le rayon du paquet ajoute d'ouverture apparente.
        let margin = (self.radius() / distance).min(1.0);
        self.axis.dot(dir) > sin_spread + margin
    }

    /// Vrai quand la boîte est entièrement du mauvais côté d'un des plans.
    ///
    /// Le test habituel du point le plus favorable: si le coin le plus avancé
    /// dans la direction du plan est encore derrière lui, tout le reste l'est.
    pub fn outside(&self, planes: &[[f32; 4]; 6]) -> bool {
        for p in planes {
            let n = Vec3::new(p[0], p[1], p[2]);
            let corner = Vec3::new(
                if n.x >= 0.0 { self.hi.x } else { self.lo.x },
                if n.y >= 0.0 { self.hi.y } else { self.lo.y },
                if n.z >= 0.0 { self.hi.z } else { self.lo.z },
            );
            if n.dot(corner) + p[3] < 0.0 {
                return true;
            }
        }
        false
    }
}

/// La partition d'un maillage, et l'ordre de faces qu'elle suppose.
#[derive(Clone, Debug, Default)]
pub struct Partition {
    pub clusters: Vec<Cluster>,
    /// Rayon moyen des paquets au moment de la mise en ordre, pour mesurer
    /// combien ils se sont élargis depuis.
    reference: f32,
}

impl Partition {
    pub fn is_empty(&self) -> bool {
        self.clusters.is_empty()
    }

    /// Remesure tous les paquets, sans toucher à l'ordre des faces.
    ///
    /// Ce qu'il faut quand les faces ont été réécrites par autre chose: les
    /// boîtes sont fausses, mais les paquets couvrent toujours le maillage, et
    /// remesurer coûte le cinquième d'une remise en ordre.
    pub fn remeasure_all(&mut self, mesh: &Mesh, target: usize) {
        self.clusters = measure(mesh, target);
        self.reference = if self.clusters.is_empty() {
            0.0
        } else {
            self.clusters.iter().map(|c| c.radius()).sum::<f32>() / self.clusters.len() as f32
        };
    }

    /// À quel point les paquets se sont élargis depuis la mise en ordre.
    ///
    /// Un sur la partition d'origine, et cela monte à mesure que la topologie
    /// dynamique brasse les faces. Passé deux, il y a plus à gagner à remettre
    /// de l'ordre qu'à continuer.
    pub fn drift(&self) -> f32 {
        if self.clusters.is_empty() || self.reference <= 0.0 {
            return 1.0;
        }
        let mean: f32 =
            self.clusters.iter().map(|c| c.radius()).sum::<f32>() / self.clusters.len() as f32;
        mean / self.reference
    }

    /// Les paquets qu'il faut dessiner, dans l'ordre.
    ///
    /// Rend des plages de faces plutôt que des paquets, et fusionne celles qui
    /// se touchent: dessiner deux paquets voisins en un seul appel coûte moins
    /// que deux appels, et sur un modèle entièrement visible cela revient à
    /// l'appel unique d'avant.
    pub fn visible_ranges(&self, planes: &[[f32; 4]; 6], eye: Option<Vec3>) -> Vec<(u32, u32)> {
        let mut out: Vec<(u32, u32)> = Vec::new();
        for c in &self.clusters {
            if c.outside(planes) {
                continue;
            }
            if eye.is_some_and(|e| c.facing_away(e)) {
                continue;
            }
            match out.last_mut() {
                // Un trou de quelques paquets est comblé plutôt que coupé:
                // redessiner deux mille triangles coûte moins cher au pilote
                // qu'un appel de dessin de plus, et à vingt-cinq millions de
                // faces le découpage produirait des centaines d'appels.
                Some((first, count)) if c.first.saturating_sub(*first + *count) <= GAP_TOLERANCE => {
                    *count = c.first + c.count - *first;
                }
                _ => out.push((c.first, c.count)),
            }
        }
        out
    }

    /// Combien de faces ces plages couvrent.
    pub fn faces_in(ranges: &[(u32, u32)]) -> u32 {
        ranges.iter().map(|(_, n)| n).sum()
    }
}

/// Rattrape la partition après un coup de brosse.
///
/// Un coup déplace quelques milliers de faces sur des millions, en ajoute et en
/// retire au passage. Tout remesurer coûterait plus cher que le coup; tout
/// réordonner, bien plus encore. Seuls sont repris les paquets qui contiennent
/// une face touchée, plus la queue de la liste quand le nombre de faces a
/// changé.
///
/// Les paquets se déforment peu à peu: la topologie dynamique déplace des faces
/// par échange avec la dernière, ce qui casse la localité un peu plus à chaque
/// coup. Les boîtes restent justes, seulement plus larges, donc le rendu reste
/// correct et devient seulement moins efficace. `drift` dit à quel point, pour
/// que l'application décide quand remettre de l'ordre.
pub fn follow(mesh: &Mesh, p: &mut Partition, dirty_faces: &[u32], target: usize) {
    let target = target.max(1);
    let n = mesh.faces.len();

    // La queue d'abord: le dernier paquet grandit ou rétrécit, et il en faut
    // de nouveaux quand des faces ont été ajoutées.
    let wanted = n.div_ceil(target);
    p.clusters.truncate(wanted);
    if let Some(last) = p.clusters.last_mut() {
        let first = last.first as usize;
        last.count = (n - first).min(target) as u32;
    }
    let mut fresh: Vec<usize> = Vec::new();
    while p.clusters.len() < wanted {
        let first = p.clusters.len() * target;
        let count = target.min(n - first);
        fresh.push(p.clusters.len());
        p.clusters.push(Cluster {
            first: first as u32,
            count: count as u32,
            lo: Vec3::ZERO,
            hi: Vec3::ZERO,
            axis: Vec3::Y,
            cos_spread: -1.0,
        });
    }

    let mut todo: Vec<usize> = dirty_faces
        .iter()
        .map(|f| *f as usize / target)
        .chain(fresh)
        .chain(p.clusters.len().checked_sub(1))
        .filter(|k| *k < p.clusters.len())
        .collect();
    todo.sort_unstable();
    todo.dedup();

    for k in todo {
        let c = &mut p.clusters[k];
        let (first, count) = (c.first as usize, c.count as usize);
        if count == 0 || first + count > n {
            continue;
        }
        let (lo, hi, axis, cos_spread) = extent(mesh, first, count);
        c.lo = lo;
        c.hi = hi;
        c.axis = axis;
        c.cos_spread = cos_spread;
    }
}

/// Entrelace les bits d'un entier de 10 bits, un sur trois.
fn spread(v: u32) -> u32 {
    let mut x = v & 0x3ff;
    x = (x | (x << 16)) & 0x030000ff;
    x = (x | (x << 8)) & 0x0300f00f;
    x = (x | (x << 4)) & 0x030c30c3;
    x = (x | (x << 2)) & 0x09249249;
    x
}

/// Code de Morton d'un point ramené dans le cube unité.
fn morton(p: Vec3) -> u32 {
    let q = (p * 1023.0).clamp(Vec3::ZERO, Vec3::splat(1023.0));
    spread(q.x as u32) | (spread(q.y as u32) << 1) | (spread(q.z as u32) << 2)
}

/// Réordonne les faces du maillage par localité et rend la partition.
///
/// Le maillage est modifié: c'est le prix à payer pour que chaque paquet occupe
/// une plage contiguë, et c'est aussi ce qui rend le tampon d'indices lisible
/// dans l'ordre où le GPU le parcourt. L'adjacence est reconstruite, la grille
/// abandonnée; les sommets ne bougent pas.
pub fn build(mesh: &mut Mesh, target: usize) -> Partition {
    let target = target.max(1);
    if mesh.faces.is_empty() {
        return Partition::default();
    }

    let (lo, hi) = mesh.bounds();
    let span = (hi - lo).max(Vec3::splat(1e-6));
    let mut keyed: Vec<(u32, u32)> = mesh
        .faces
        .par_iter()
        .enumerate()
        .map(|(i, tri)| {
            let c = (mesh.pos[tri[0] as usize]
                + mesh.pos[tri[1] as usize]
                + mesh.pos[tri[2] as usize])
                / 3.0;
            (morton((c - lo) / span), i as u32)
        })
        .collect();
    keyed.par_sort_unstable();

    let faces: Vec<[u32; 3]> = keyed
        .par_iter()
        .map(|(_, i)| mesh.faces[*i as usize])
        .collect();
    // The faces have a new order, so every face index in the ring has to point
    // at its new slot. Rebuilding the ring from the whole face array costs a
    // read of every face per worker; remapping the old indices in place is one
    // pass over the ring, which on a sculpted mesh is a handful of entries per
    // vertex. The ring does not keep its entries sorted by face index, which
    // nothing that reads it relies on.
    let mut perm: Vec<u32> = Vec::with_capacity(mesh.faces.len());
    perm.resize(mesh.faces.len(), 0);
    for (new_i, &(_, old_i)) in keyed.iter().enumerate() {
        perm[old_i as usize] = new_i as u32;
    }
    mesh.faces = faces;
    for fl in &mut mesh.vfaces {
        for f in fl.iter_mut() {
            *f = perm[*f as usize];
        }
    }
    mesh.invalidate_accel();
    mesh.mark_fully_dirty();

    let clusters = measure(mesh, target);
    let reference = if clusters.is_empty() {
        0.0
    } else {
        clusters.iter().map(|c| c.radius()).sum::<f32>() / clusters.len() as f32
    };
    Partition { clusters, reference }
}

/// Découpe la liste de faces en paquets et mesure chacun.
///
/// Séparé de la mise en ordre, parce qu'un maillage déjà ordonné n'a besoin que
/// de ceci: après un coup de brosse, les positions ont bougé mais l'ordre des
/// faces tient encore.
pub fn measure(mesh: &Mesh, target: usize) -> Vec<Cluster> {
    let target = target.max(1);
    let n = mesh.faces.len();
    (0..n.div_ceil(target))
        .into_par_iter()
        .map(|k| {
            let first = k * target;
            let count = target.min(n - first);
            let (lo, hi, axis, cos_spread) = extent(mesh, first, count);
            Cluster { first: first as u32, count: count as u32, lo, hi, axis, cos_spread }
        })
        .collect()
}

/// Boîte et cône des normales d'une tranche de faces.
fn extent(mesh: &Mesh, first: usize, count: usize) -> (Vec3, Vec3, Vec3, f32) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let mut sum = Vec3::ZERO;
    for tri in &mesh.faces[first..first + count] {
        for &v in tri {
            let p = mesh.pos[v as usize];
            lo = lo.min(p);
            hi = hi.max(p);
        }
        sum += face_normal(mesh, tri);
    }
    // L'axe est la moyenne des normales; l'ouverture est celle qui contient la
    // plus écartée d'entre elles.
    let axis = sum.normalize_or(Vec3::Y);
    let mut cos_spread = 1.0f32;
    for tri in &mesh.faces[first..first + count] {
        cos_spread = cos_spread.min(axis.dot(face_normal(mesh, tri)));
    }
    (lo, hi, axis, cos_spread)
}

fn face_normal(mesh: &Mesh, tri: &[u32; 3]) -> Vec3 {
    let a = mesh.pos[tri[0] as usize];
    let b = mesh.pos[tri[1] as usize];
    let c = mesh.pos[tri[2] as usize];
    (b - a).cross(c - a).normalize_or(Vec3::Y)
}

/// Remesure les paquets qui contiennent une des faces citées.
///
/// Un coup de brosse touche quelques paquets sur des milliers. Tout remesurer
/// coûterait plus cher que le coup lui-même.
pub fn remeasure(mesh: &Mesh, clusters: &mut [Cluster], faces: &[u32], target: usize) {
    if clusters.is_empty() {
        return;
    }
    let target = target.max(1);
    let mut touched: Vec<u32> = faces.iter().map(|f| *f / target as u32).collect();
    touched.sort_unstable();
    touched.dedup();

    for k in touched {
        let Some(c) = clusters.get_mut(k as usize) else {
            continue;
        };
        let (first, count) = (c.first as usize, c.count as usize);
        if first + count > mesh.faces.len() {
            continue;
        }
        let (lo, hi, axis, cos_spread) = extent(mesh, first, count);
        c.lo = lo;
        c.hi = hi;
        c.axis = axis;
        c.cos_spread = cos_spread;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives;

    /// Tout doit rester exactement là où c'était: la partition réordonne les
    /// faces, elle ne change pas la forme.
    #[test]
    fn partitioning_keeps_the_model_intact() {
        let mut m = primitives::icosphere(4);
        let faces_before = m.face_count();
        let verts_before = m.vert_count();
        let mut sorted_before: Vec<[u32; 3]> = m
            .faces
            .iter()
            .map(|t| {
                let mut a = *t;
                a.sort_unstable();
                a
            })
            .collect();
        sorted_before.sort_unstable();

        let p = build(&mut m, 256);

        assert_eq!(m.face_count(), faces_before);
        assert_eq!(m.vert_count(), verts_before);
        let mut sorted_after: Vec<[u32; 3]> = m
            .faces
            .iter()
            .map(|t| {
                let mut a = *t;
                a.sort_unstable();
                a
            })
            .collect();
        sorted_after.sort_unstable();
        assert_eq!(sorted_before, sorted_after, "les faces ne sont plus les mêmes");
        assert!(!p.is_empty());
    }

    /// Les paquets doivent couvrir toutes les faces, une seule fois chacune.
    #[test]
    fn the_clusters_cover_every_face_once() {
        let mut m = primitives::icosphere(4);
        let p = build(&mut m, 300);
        let mut next = 0u32;
        for c in &p.clusters {
            assert_eq!(c.first, next, "un trou ou un recouvrement entre paquets");
            next += c.count;
        }
        assert_eq!(next as usize, m.face_count());
    }

    /// Une boîte doit contenir ses faces, sinon un paquet visible sera écarté.
    #[test]
    fn every_box_contains_its_faces() {
        let mut m = primitives::icosphere(4);
        let p = build(&mut m, 300);
        for c in &p.clusters {
            for tri in &m.faces[c.first as usize..(c.first + c.count) as usize] {
                for &v in tri {
                    let q = m.pos[v as usize];
                    assert!(
                        q.cmpge(c.lo - Vec3::splat(1e-5)).all()
                            && q.cmple(c.hi + Vec3::splat(1e-5)).all(),
                        "un sommet est hors de la boîte de son paquet"
                    );
                }
            }
        }
    }

    /// Le rangement par localité doit resserrer les paquets: c'est toute la
    /// raison de réordonner.
    ///
    /// La comparaison se fait contre un ordre mélangé, pas contre celui que la
    /// primitive donne. Une icosphère sort déjà bien rangée de sa subdivision
    /// récursive, alors se mesurer à elle ne dirait rien du cas qui compte: un
    /// maillage importé, ou sorti d'un remaillage.
    #[test]
    fn locality_makes_the_boxes_smaller() {
        let mut shuffled = primitives::icosphere(5);
        // Mélange reproductible: un pas de générateur congruentiel.
        let n = shuffled.faces.len();
        let mut seed = 12345u64;
        for i in (1..n).rev() {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let j = (seed >> 33) as usize % (i + 1);
            shuffled.faces.swap(i, j);
        }
        shuffled.rebuild_adjacency();
        let mut ordered = shuffled.clone();

        let before = measure(&shuffled, 256);
        let after = build(&mut ordered, 256);

        let mean = |cs: &[Cluster]| -> f32 {
            cs.iter().map(|c| c.radius()).sum::<f32>() / cs.len() as f32
        };
        let (a, b) = (mean(&after.clusters), mean(&before));
        assert!(a < b * 0.5, "rayon moyen {a} contre {b}, le rangement n'a rien resserré");
    }

    /// Rien ne doit être écarté quand tout est devant la caméra, et tout doit
    /// l'être quand rien ne l'est.
    #[test]
    fn culling_keeps_what_is_in_view() {
        let mut m = primitives::icosphere(4);
        let p = build(&mut m, 256);

        // Six plans qui enferment largement le modèle.
        let wide = [
            [1.0, 0.0, 0.0, 10.0],
            [-1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, 1.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ];
        let ranges = p.visible_ranges(&wide, None);
        assert_eq!(Partition::faces_in(&ranges) as usize, m.face_count());
        // Tout visible et contigu: un seul appel de dessin.
        assert_eq!(ranges.len(), 1);

        // Un demi-espace très loin devant: plus rien.
        let far = [
            [1.0, 0.0, 0.0, -100.0],
            [-1.0, 0.0, 0.0, 200.0],
            [0.0, 1.0, 0.0, 200.0],
            [0.0, -1.0, 0.0, 200.0],
            [0.0, 0.0, 1.0, 200.0],
            [0.0, 0.0, -1.0, 200.0],
        ];
        assert!(p.visible_ranges(&far, None).is_empty());
    }

    /// Un huitième du modèle demandé doit en écarter la plus grande part.
    ///
    /// Le compte exact dépend de la taille des paquets, et un paquet à cheval
    /// sur la limite doit être gardé: ce qu'on vérifie est qu'il en reste une
    /// petite fraction, pas un chiffre précis.
    #[test]
    fn culling_actually_rejects_something() {
        let mut m = primitives::icosphere(6);
        let p = build(&mut m, 512);
        // Ne garde que l'octant x, y, z tous positifs.
        let octant = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0, 10.0],
            [0.0, -1.0, 0.0, 10.0],
            [0.0, 0.0, -1.0, 10.0],
        ];
        let kept = Partition::faces_in(&p.visible_ranges(&octant, None)) as usize;
        assert!(
            kept < m.face_count() / 2,
            "gardé {kept} sur {}, le tri ne sert à rien",
            m.face_count()
        );
        assert!(kept > 0, "tout a été écarté, y compris ce qui est visible");
    }

    /// Un paquet ne doit jamais être écarté quand une seule de ses faces est
    /// tournée vers l'oeil: le trou se verrait.
    #[test]
    fn facing_away_never_hides_a_visible_face() {
        let mut m = primitives::icosphere(5);
        let p = build(&mut m, 256);
        for eye in [
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(2.0, 2.0, 2.0),
            Vec3::new(-1.5, 0.3, 0.9),
        ] {
            for c in &p.clusters {
                if !c.facing_away(eye) {
                    continue;
                }
                for tri in &m.faces[c.first as usize..(c.first + c.count) as usize] {
                    let n = face_normal(&m, tri);
                    let a = m.pos[tri[0] as usize];
                    assert!(
                        n.dot((a - eye).normalize()) > 0.0,
                        "une face de face a été écartée"
                    );
                }
            }
        }
    }

    /// Sur une forme fermée, la moitié doit partir.
    #[test]
    fn facing_away_rejects_about_half_a_sphere() {
        let mut m = primitives::icosphere(6);
        let p = build(&mut m, 512);
        let eye = Vec3::new(0.0, 0.0, 3.0);
        let away: u32 = p
            .clusters
            .iter()
            .filter(|c| c.facing_away(eye))
            .map(|c| c.count)
            .sum();
        let part = away as f32 / m.face_count() as f32;
        assert!(part > 0.25, "seulement {:.0}% écarté", part * 100.0);
    }

    /// La partition doit rester juste quand la topologie change sous elle:
    /// aucune face hors de sa boîte, et le compte doit suivre.
    #[test]
    fn following_keeps_the_partition_honest() {
        use crate::{BrushKind, Sculptor, StrokeInput};

        let mut m = primitives::icosphere(4);
        let mut p = build(&mut m, 256);
        let mut s = Sculptor::new(m);
        s.symmetry = false;
        s.dyntopo_enabled = true;
        s.dyntopo.detail = s.mesh().mean_edge_len() * 0.4;
        s.brush.kind = BrushKind::Draw;
        s.brush.radius = 0.35;
        s.brush.strength = 0.6;

        s.begin_stroke();
        for i in 0..6 {
            let at = Vec3::new(i as f32 * 0.05, 1.0, 0.0).normalize();
            s.stroke(&StrokeInput { point: at, normal: at, ..Default::default() });
            let (dirty_verts, dirty_faces, _) = s.take_dirty();
            let _ = dirty_verts;
            follow(s.mesh(), &mut p, &dirty_faces, 256);
        }
        s.end_stroke();

        let m = s.mesh();
        assert!(m.face_count() > 5120, "la topologie n'a pas bougé");
        let covered: u32 = p.clusters.iter().map(|c| c.count).sum();
        assert_eq!(covered as usize, m.face_count(), "des faces sans paquet");

        let mut next = 0u32;
        for c in &p.clusters {
            assert_eq!(c.first, next);
            next += c.count;
            for tri in &m.faces[c.first as usize..(c.first + c.count) as usize] {
                for &v in tri {
                    let q = m.pos[v as usize];
                    assert!(
                        q.cmpge(c.lo - Vec3::splat(1e-4)).all()
                            && q.cmple(c.hi + Vec3::splat(1e-4)).all(),
                        "une face est hors de la boîte de son paquet"
                    );
                }
            }
        }
    }

    #[test]
    fn remeasuring_follows_a_moved_vertex() {
        let mut m = primitives::icosphere(4);
        let mut p = build(&mut m, 256);
        let before = p.clusters[0];

        // Pousse un sommet du premier paquet très loin.
        let v = m.faces[0][0] as usize;
        m.pos[v] += Vec3::new(0.0, 5.0, 0.0);
        remeasure(&m, &mut p.clusters, &[0], 256);

        assert!(p.clusters[0].hi.y > before.hi.y + 1.0, "la boîte n'a pas suivi");
        assert_eq!(p.clusters[1], measure(&m, 256)[1], "un paquet voisin a bougé");
    }
}
