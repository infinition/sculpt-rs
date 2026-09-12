# Atelier de sculpture

## Brushes et aperçu

L'onglet **Brush** regroupe le rayon, la force, l'espacement des touches,
la pression, le falloff, l'alpha et les options propres à l'outil. Le hover
montre l'alpha multipliée par le falloff sur le plan tangent de la surface.
Son opacité est réglable. Il s'agit d'une empreinte projetée : elle ne représente
pas exactement la déformation future et ne suit pas les plis par un test de profondeur.

Les 16 outils incluent Clay, Draw, Flatten, Smooth, Pinch, Crease, Inflate,
Move, Drag, Twist, Scale, Paint, Smudge, Blur, Fill et Mask. Shift donne accès
temporairement à Smooth ; Ctrl inverse temporairement les outils compatibles.
Chaque outil conserve ses réglages lors du changement d'outil et du redémarrage.

Dans la bibliothèque, le menu **…** d'une brush permet de la renommer, dupliquer,
réordonner, remplacer par les réglages courants ou supprimer. Son icône peut être
un pictogramme d'outil ou une image importée, normalisée en 64 × 64 pixels.
Le menu est également accessible au clic droit.

Exporter en **.sculptbrush** pour transporter les réglages, les icônes et les
alphas utilisées ensemble. Le format historique **.brushes** reste disponible,
mais référence ses alphas par nom sans embarquer leurs images. Les collisions
de noms d'alpha sont résolues lors de l'import d'un pack.

## Matériaux, objets et rendu

L'onglet **Materials** contient une bibliothèque éditable de couleurs,
roughness et metalness. Choisir un matériau configure l'outil Paint et active
le rendu PBR. La peinture est stockée aux sommets du maillage.
**Add mesh to scene** ajoute un OBJ, PLY ou STL à la scène existante.
Les textures importées dans la section Stamp servent d'empreintes de brush.

Le rendu propose Clay avec éclairage de studio, Matcap et PBR, plus AO,
anticrénelage et contrôles de tonalité. Ce jalon n'ajoute pas de peinture UV,
de bibliothèque HDRI, de SSGI, de couches de sculpture ou de multirésolution.

## Organisation et périphériques

Les sections peuvent quitter le panneau latéral pour devenir des fenêtres
flottantes dans l'application. Elles restent dans la fenêtre principale ;
ce ne sont pas des fenêtres natives à déplacer sur un autre écran.
Le panneau UI règle notamment l'échelle, les dimensions des contrôles,
les couleurs, les docks et les contrôles flottants. Le mode tactile agrandit
les commandes. Les gestes à deux doigts assurent la navigation.

La configuration est enregistrée à la fermeture normale, ou avec
**UI > Save workspace now** : disposition, raccourcis, réglages de rendu,
outils, brushes, icônes, alphas et matériaux. Les chemins par système sont
dans [release.md](release.md). La scène se sauvegarde séparément ;
il n'y a pas d'autosauvegarde de récupération après crash dans ce jalon.

Les transitions tactile/stylet/souris, les annulations et la perte de focus
sont couvertes par des tests de logique. Une validation sur de vrais écrans
multitouch et tablettes reste nécessaire, notamment pour les appareils qui
ne distinguent pas le stylet du doigt par les événements de pression.

## Sculptures très denses

`sculpt-app --stress-40m` prépare un objet de 41 943 040 triangles.
Le chargement et la construction des index sont synchrones et peuvent prendre
plusieurs secondes. Le démarrage choisit une brush de rayon 0,05 en topologie
fixe. La dyntopo progressive limite le travail de subdivision par touche ;
elle peut nécessiter plusieurs passages pour atteindre le détail demandé.

La taille de la région touchée, la symétrie et la topologie dynamique ont
autant d'importance que le total de polygones. Voir les mesures reproductibles,
leur périmètre et les limites restantes dans [performance-audit.md](performance-audit.md).
