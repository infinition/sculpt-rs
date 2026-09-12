# Audit de latence, 7 septembre 2026

Le problème principal observé est le travail CPU sur le chemin interactif. Le
langage Rust et la RTX ne compensent pas un parcours complet du modèle à chaque
recherche du curseur, ni plusieurs budgets de sculpture exécutés avant une image.

## Ce qui a été vérifié

Machine locale : Intel i5-9600K, 6 cœurs / 6 threads, RTX 4070 Ti 12 Go,
pilote 610.47, backend Vulkan. Compilation release, LTO thin. Référence source :
`77eac7856cfbbcc888a6751743c8fe714008c314`.

La référence a été extraite dans `target/perf-baseline`, compilée avec une copie
du même `Cargo.lock` et exécutée séparément de la version modifiée. Les premières
mesures pendant des compilations concurrentes ont été écartées. Les relevés bruts
restent sur la station locale et ne sont pas versionnés ; les chiffres retenus
sont repris dans les tableaux ci-dessous.

`interaction` utilise une icosphère, une brosse Clay de rayon 0,15, sans symétrie,
60 coups en mouvement par mode, avec un plafond de 40 millions de sommets pour
ne pas désactiver involontairement les subdivisions. La recherche du curseur,
la préparation de l'index, le moteur et la maintenance des paquets sont mesurés
séparément. Le test « no grid » reste volontairement présent comme témoin ;
l'application prépare désormais l'index avant d'afficher le curseur.

`viewport` dessine une caméra tournante, en 1920 × 1080, avec élimination des
paquets invisibles, Matcap, MSAA 1 ou 4 et occlusion activée ou non. Dix images de
chauffe précèdent 60 mesures. Chaque mesure attend réellement la fin du GPU.
Elle inclut la soumission CPU mais exclut egui, le picking, les brosses et la
présentation à l'écran. Ce ne sont donc pas les FPS de l'application.

## Causes et modifications

1. **Index de picking absent après réorganisation.** `cluster::build` réordonne
   les faces et invalide la grille. Le viewport pouvait ensuite utiliser la
   recherche exhaustive jusqu'au début d'un trait. `State::update_partitions`
   prépare maintenant l'index des objets visibles pendant le chargement ou la
   réécriture du modèle. Le mode voxel n'est pas couvert par cette préparation.

2. **Un rayon qui manque la boîte demandait un scan complet.** Le `?` de
   `Grid::raycast` renvoyait `None`, qui signifie « utiliser le fallback », alors
   que `Some(None)` signifie « aucun impact ». Ce cas est corrigé. Le parcours
   déduplique aussi les buckets visités : deux pas voisins ne doivent pas tester
   à nouveau les mêmes triangles. Une limite de parcours atteinte demande en
   revanche une recherche complète pour conserver une réponse exacte.

3. **Budget par événement au lieu de budget par image.** Les mouvements souris
   et stylet sont maintenant regroupés et consommés avant le rendu. Le dernier
   échantillon conserve position et pression ; le relâchement consomme la fin du
   trait avant de fermer l'historique. La mesure du premier coup initialise
   immédiatement l'estimation de coût. Le cercle de brosse ne demande plus un
   picking inutile pendant la navigation.

4. **Maintenance coûteuse des cellules denses.** Les suppressions dans la grille
   utilisaient une recherche linéaire dans le bucket. Chaque élément mémorise
   désormais son emplacement interne ; déplacement et suppression sont en O(1).
   Cela ajoute 4 octets par sommet et par face, hors capacités réservées.

5. **Maintenance incorrecte des faces déformées.** Une face peut changer de
   cellule ou de rayon même si ses sommets restent dans leurs cellules. Une
   déformation change aussi ses boîtes et cônes de visibilité sans modifier ses
   indices. Ces faces sont maintenant signalées et refittées. Les paquets
   étendent leurs bornes de façon conservative à partir des seules faces touchées,
   au lieu de relire 2048 faces par paquet touché. Une borne peut devenir moins
   serrée, mais elle ne doit pas cacher une face visible.

6. **Raffinement dyntopo.** Le tri intégral des candidats est remplacé par une
   sélection du préfixe utile. Une file de priorité met à jour les arêtes autour
   de chaque nouveau sommet, au lieu de refaire une recherche complète de la
   région entre trois passes. Le nouveau réglage **Progressive refinement**,
   activé par défaut, limite les subdivisions à 1024 par coup, contre un plafond
   de 3072 auparavant. Le désactiver permet 3072 subdivisions. La longueur
   d'arête cible reste la même ; le nombre de coups nécessaire pour l'atteindre
   peut augmenter. L'ordre du raffinement change : les résultats ne sont pas
   identiques sommet pour sommet à ceux de l'ancien algorithme.

7. **À-coup de la première subdivision locale.** L'index et les tableaux du
   maillage réservent une marge pendant leur préparation : environ 12,5 % des
   sommets, bornée entre 16 384 et 1 048 576 sommets, et deux fois cette quantité
   de faces. Les canaux dormants restent dormants. Ce travail augmente le coût
   de préparation et la mémoire réservée ; il évite que le premier ajout fasse
   grandir simultanément plusieurs tableaux géants.

8. **Mesures trompeuses.** Le compteur ne présente plus `1 / temps CPU` comme des
   FPS GPU. Les images coûteuses ne sont plus exclues et l'upload vaut zéro quand
   aucune géométrie n'est envoyée. Le benchmark historique `offscreen` démarre
   maintenant le chronomètre avant la soumission et attend chaque image ; il
   mesurait auparavant seulement l'attente résiduelle en fin de lot. `dab_cost`
   inclut maintenant le refit, jusque-là hors de son total.

## Pourquoi Nomad peut être plus fluide et plus beau

Les fichiers d'analyse disponibles dans le dossier Nomad décrivent notamment
des canaux quantifiés, des traitements parallèles et du post-traitement. Mais
`hot_functions.txt` classe des fonctions par taille de code, pas par temps CPU.
Décompiler des fonctionnalités ne fournit ni les profils d'exécution, ni le
coût des allocations, ni la politique d'ordonnancement des interactions.

La documentation officielle de Nomad expose deux différences concrètes :

- Un niveau moins détaillé peut être affiché pendant la navigation, via le
  [seuil de basse résolution](https://nomadsculpt.com/manual/settings#low-resolution-threshold).
- L'image au repos se raffine par accumulation de plusieurs images ; la
  résolution et les échantillons des effets sont réglables. Nomad propose aussi
  réflexions, illumination indirecte, occlusion et tone mapping.
  [Documentation du post-traitement](https://nomadsculpt.com/manual/postprocess).

Sculpt-rs possède déjà Matcap, tone mapping et occlusion, mais son PBR actuel
repose sur trois lumières simples et ne possède pas cette accumulation temporelle.
Ces différences expliquent des capacités visuelles différentes ; elles ne
constituent pas une comparaison chronométrée de deux scènes identiques. Aucun
benchmark Nomad en conditions identiques n'a été réalisé pendant cet audit.

## Limites et prochain changement d'architecture

Cette correction n'établit pas « 60 FPS garantis sur tous les modèles ».

- Une grosse brosse touche davantage de sommets à mesure que la densité augmente.
  Le budget de subdivisions ne borne pas le coût de la déformation, du lissage,
  de la sélection ni des normales. Une seule dab peut encore dépasser 16,7 ms.
- La maintenance correcte des faces ajoute du travail aux brosses sans dyntopo :
  comparer leur ancien temps sans tenir compte des bornes périmées serait trompeur.
- La file de pointeur conserve au plus 16 échantillons et simplifie les segments
  les moins significatifs (géométrie et pression) sous charge. Les mouvements
  inférieurs à l'espacement de la brush ne déclenchent plus une dab entière.
  Le dernier point est appliqué au relâchement. Cela réduit le travail inutile,
  sans garantir la conservation exacte de toute trajectoire à fréquence arbitraire.
- Les tableaux restent contigus. Épuiser leur capacité réservée, reconstruire
  l'index après un changement important de rayon ou réécrire la topologie peut
  encore produire un à-coup. Les objets cachés redevenus visibles peuvent aussi
  demander une préparation.
- Les scènes testées sont des sphères. Une scène avec beaucoup d'objets, du
  wireframe, de très gros triangles mélangés à des petits, des surplombs ou une
  grande brosse doit être mesurée séparément. Le mode voxel n'a pas été optimisé.
- Les tests automatisés ne remplacent pas une validation manuelle au stylet
  avec le modèle réellement utilisé.

Le changement structurel à viser est **un moteur par blocs spatiaux**, avec des
coûts proportionnels aux blocs touchés et visibles, et un budget d'interaction
explicite. Ordre conseillé à partir des mesures obtenues :

1. Stockage par blocs avec réserve locale, index hiérarchique de picking
   indépendant du rayon de brosse, normales et uploads incrémentaux par bloc.
   Cela supprime les réallocations globales et réduit les replis sur tout le mesh.
2. Pour les grosses régions, déformation et normales à topologie fixe sur GPU,
   avec synchronisation explicite des blocs dont CPU, picking et undo ont besoin.
   La topologie dynamique reste une opération distincte avec budget ; déplacer
   une boucle CPU sur le GPU sans résoudre les transferts ne suffit pas.
3. Niveaux de détail pour les scènes qui saturent effectivement le rendu, puis
   éclairage HDR et effets accumulés au repos pour la qualité visuelle. Les
   mesures actuelles ne justifient pas de commencer par réécrire le rasteriseur.

## Reproduire

Depuis la racine du projet :

```powershell
cargo test --release -p sculpt-core
cargo test --release -p sculpt-app --bin sculpt-app
cargo run --release -p sculpt-core --example interaction -- 9
cargo run --release -p sculpt-core --example interaction -- 10
cargo run --release -p sculpt-app --example viewport -- 9
cargo run --release -p sculpt-app --example viewport -- 10
cargo run --release -p sculpt-app --example offscreen
cargo run --release -p sculpt-app
```

Éviter d'exécuter les benchmarks en parallèle d'une compilation ou d'un autre
benchmark. Les niveaux 9 et 10 correspondent à 5 242 880 et 20 971 520 triangles.


## Mesures du 11 septembre 2026 : 41,9 millions de triangles

Machine mesurée : i5-9600K (6 coeurs / 6 threads), RTX 4070 Ti 12 Go,
32 Go de RAM, Vulkan, pilote NVIDIA 610.47. Le maillage est **un seul objet** :
une sphère UV fermée de 20 971 522 sommets et **41 943 040 triangles**.
Ce n'est pas une collection d'instances d'un petit objet.

Le générateur partage désormais les pôles et la couture sans passer par un
hash de soudure de millions de points ; l'orientation des triangles est corrigée
vers l'extérieur. Le maillage dense reste entièrement stocké en RAM et sur GPU.

| Chemin CPU, Clay rayon 0,15, sans symétrie | Premier relevé p50 | Après partage du refit p50 / p95 |
| --- | ---: | ---: |
| Picking avec index | 1,48 ms | 1,37 / 1,75 ms |
| Dab + refit + partition, topologie fixe | 25,67 ms | 14,69 / 16,36 ms |
| Dab + refit + partition, dyntopo progressif | 40,19 ms | 24,73 / 32,52 ms |

Ces trois relevés viennent de la même session locale du 11 septembre.
La préparation de l'index et des réserves coûte encore environ **1,15 seconde**
sur ce modèle ; elle est hors des temps par dab. Le premier relevé possédait
**déjà** les corrections du 7 septembre : la comparaison ne représente pas
l'écart total par rapport à HEAD. Les variantes dyntopo peuvent produire un
nombre final de triangles légèrement différent (réductions flottantes parallèles).

Le rendu seul à 1920x1080, caméra tournante, Matcap, AO et MSAA 4, mesure
**2,81 ms p50 / 3,90 ms p95**. Environ 12,6 millions de triangles sont soumis en
moyenne après culling de blocs. Ce résultat comprend la soumission et l'attente
GPU, mais pas la brush, egui, la présentation, la synchronisation écran ni la
latence du périphérique. Il ne constitue donc pas « 356 FPS dans l'application ».
Le test complet de sculpture est disponible avec `viewport --sculpt`.

À 5,24 millions, un passage avant/après le même jour donne : picking sur modèle
1,66 -> 0,38 ms ; picking manqué 24,48 -> moins de 0,001 ms ; dyntopo progressif
20,65 -> 10,91 ms (p50). Le nombre de subdivisions par dab est réduit dans le
mode progressif : la densité finale n'est pas identique. En topologie fixe,
le coût passe de 1,77 à 2,85 ms dans ce relevé : la version originale omettait
une maintenance indispensable des bornes après déplacement. Les normales et le
refit partagé ont ensuite été optimisés davantage.

### Changements supplémentaires validables

- Normales et index spatial partagent les faces incidentes uniques ; leur tri
  volumineux et le refit de blocs sont parallélisés. Un test compare les normales
  locales à un recalcul intégral après sculpture fixe et dynamique.
- Les faces déplacées continuent de mettre à jour le culling, mais leurs indices
  ne sont plus envoyés au GPU en topologie fixe.
- Les pixels noirs d'une alpha et les sommets complètement protégés sont exclus
  du journal de déformation, du refit et des uploads de la brush.
- Les événements de souris synthétiques ne remplacent plus la pression tactile.
  Perte de focus, annulation d'un contact et Ctrl temporaire sont traités.
- L'espacement des dabs est réglable et tient compte du rayon projeté. Une file
  bornée protège les angles et les variations de pression ; le point final est
  conservé. Le nombre d'événements matériels ne doit pas multiplier les dabs
  pour une même translation rectiligne.
- La projection place le centre du modèle entre les panneaux, et le picking
  emploie la même matrice. Le rayon de hover utilise la profondeur de vue.

### Reproduire sur le modèle à 41,9 millions

```powershell
cargo run --release --locked -p sculpt-core --example interaction -- 40m
cargo run --release --locked -p sculpt-app --example viewport -- 40m
cargo run --release --locked -p sculpt-app --example viewport -- 40m --sculpt --radius=0.15
cargo run --release --locked -p sculpt-app --example viewport -- 40m --sculpt --radius=0.05
cargo run --release --locked -p sculpt-app -- --stress-40m
```

Le lancement interactif `--stress-40m` utilise une brush de rayon 0,05 et la
topologie fixe. La symétrie reste réglable dans l'interface. Activer la dyntopo
ou augmenter le rayon change fortement la charge : ces réglages ne sont pas
interchangeables dans un chiffre de performance.
