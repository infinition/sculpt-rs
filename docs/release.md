# Distributions natives

Le workflow Build produit des archives Windows x64, Linux x64, macOS Apple Silicon
et macOS Intel. Le workflow Release réutilise exactement ce build pour les tags
`v*`, puis joint les quatre archives et leurs SHA-256 à une release brouillon.
Le lancement manuel prépare les artefacts sans publier de release.

- Windows : extraire le ZIP puis lancer `sculpt-app.exe`.
- Linux : extraire le tar.gz puis lancer `./sculpt-app`. Un pilote Vulkan et un
  environnement de bureau X11/Wayland sont nécessaires. Le build utilise Ubuntu
  22.04 ; la distribution cible doit fournir glibc 2.35 ou ultérieure.
- macOS : extraire puis déplacer `Sculpt RS.app` dans Applications. Choisir ARM64
  pour Apple Silicon, x86_64 pour Intel. Le paquet est signé ad hoc, sans certificat
  Developer ID ni notarisation Apple ; Gatekeeper peut demander une autorisation
  d'ouverture dans les réglages de sécurité.

Les versions Windows ne sont pas signées avec un certificat Authenticode.

Les réglages et la bibliothèque de brushes sont enregistrés à la fermeture :
`%APPDATA%/sculpt-rs/workspace.json` sur Windows,
`~/Library/Application Support/sculpt-rs/workspace.json` sur macOS,
`${XDG_CONFIG_HOME:-~/.config}/sculpt-rs/workspace.json` sur Linux.
Le bouton **UI > Save workspace now** permet d'enregistrer avant la fermeture.
`SCULPT_RS_CONFIG` peut désigner un autre fichier de configuration.
Les scènes restent enregistrées séparément avec **Save scene**.

Pour préparer localement une archive Windows après les tests :

```powershell
cargo test --release --locked --workspace
cargo build --release --locked -p sculpt-app
python scripts/package.py --target x86_64-pc-windows-msvc --version v0.2.0 --binary target/release/sculpt-app.exe
```

La création de paquets macOS doit tourner sur macOS (iconutil et codesign).
Les tests CPU s'exécutent sur les quatre systèmes dans CI. Les vérifications GPU
restent distinctes : les runners hébergés ne représentent pas une station de sculpture.
