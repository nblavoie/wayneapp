# Wayne

Lanceur façon Alfred pour Windows : **Alt+Espace**, tu tapes, **Entrée**.

Rust + Win32 natif (GDI/GDI+), sans framework ni WebView : un seul exécutable d'environ 460 Ko, résident, affichage instantané.

## Ce qu'il lance

- **Applications** : tout ce qu'affiche le menu Démarrer (apps classiques + Microsoft Store), réindexé en arrière-plan.
- **Dossiers de haut niveau** : `f` Explorateur de fichiers, `i` Images, `d` Téléchargements, `b` Bureau, `m` Musique, `v` Vidéos, `h` Dossier personnel, `docs` Documents.
- **Commandes** : « Quitter Wayne », « Lancer Wayne au démarrage de Windows ».

## Clavier

| Touche | Action |
|---|---|
| Alt+Espace | Afficher / masquer |
| ↑ ↓ | Sélection |
| Entrée | Lancer |
| Ctrl+Entrée | Afficher dans l'Explorateur |
| Ctrl+1…6 | Lancer la ligne N |
| Tab | Compléter avec le nom sélectionné |
| Échap | Masquer |

## Zone de notification

L'icône **W** de la zone de notification : clic gauche pour ouvrir ou fermer Wayne, clic droit pour le menu (Ouvrir, Lancer au démarrage de Windows, Quitter). Windows 11 range les nouvelles icônes dans le menu caché (^) : glisse-la dans la barre pour la garder visible.

## Installateur

```bash
powershell -ExecutionPolicy Bypass -File build-installer.ps1
```

Produit `dist\Wayne-Setup-<version>.exe` (un seul fichier, sans dépendance) et son `.sha256`. Installation par utilisateur, sans droits admin ; `/S` pour une installation silencieuse, désinstallation depuis « Applications installées ».

## Prédiction

Chaque lancement est enregistré dans `%APPDATA%\Wayne\history.tsv`. Wayne apprend localement (décroissance de 30 jours) :

- ce que tu choisis après avoir tapé « p », « pi »… ;
- tes apps fréquentes ;
- tes habitudes selon l'heure et le jour (Bayes naïf lissé).

Requête vide → les apps les plus probables maintenant. Requête tapée → le score texte est ajusté par ces probabilités.

## Compiler

```bash
cargo build --release
```

L'exécutable est `target\release\wayne.exe`. `--hidden` démarre sans afficher la fenêtre (utilisé par le démarrage automatique).

## Licence

Wayne est un logiciel libre, distribué sous licence [GNU GPL v3.0 ou ultérieure](LICENSE). Code source : https://github.com/nblavoie/wayneapp
