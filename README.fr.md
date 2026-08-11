<div align="center">

<img src="frontend/public/favicon.svg" width="72" alt="Dub Studio"/>

# Dub Studio

**Studio de doublage vidéo par IA, gratuit et hors ligne, pour Windows —— redouble n'importe quelle vidéo dans une autre langue avec voix clonée, sous-titres traduits et localisation du texte à l'écran. 100% local, zéro Python : un `.exe` natif (Rust + C++/CUDA) ; tous les modèles et moteurs se téléchargent d'un bouton.**

[![License](https://img.shields.io/github/license/timoncool/dub-studio?style=flat-square)](LICENSE)
[![Stars](https://img.shields.io/github/stars/timoncool/dub-studio?style=flat-square)](https://github.com/timoncool/dub-studio/stargazers)
[![Latest release](https://img.shields.io/github/v/release/timoncool/dub-studio?include_prereleases&style=flat-square)](https://github.com/timoncool/dub-studio/releases)
[![Downloads](https://img.shields.io/github/downloads/timoncool/dub-studio/total?style=flat-square)](https://github.com/timoncool/dub-studio/releases)

[English](README.md) · [Русский](README.ru.md) · [中文](README.zh.md) · [Español](README.es.md) · [Português](README.pt.md) · **Français**

### [🌐 Démo en ligne et showcase avant/après →](https://timoncool.github.io/dub-studio/)



</div>

## En action

**[▶ Voir le showcase avant/après sur le site →](https://timoncool.github.io/dub-studio/#showcase)** — de vrais clips doublés de bout en bout sur un GPU local : vidéos, modes et langues différents.

| ![dub](docs/shots/mode-dub-ru.png) | ![voiceover](docs/shots/mode-voiceover-es.png) | ![dub CJK](docs/shots/mode-dub-zh.png) |
|:--:|:--:|:--:|
| 🎙️ **Doublage** · EN→RU | 🗣️ **Voix off** · EN→ES | 🈶 **Doublage** · 中文 à l'image |
| ![subtitles](docs/shots/mode-subtitles-ru.png) | ![widescreen](docs/shots/mode-dub-cinema-fr.png) | ![transcript](docs/shots/mode-transcribe-pt.png) |
| 📝 **Sous-titres** · langue d'origine | 🎬 **Doublage** · large 16:9 | 🔤 **Transcription** · diarisation |

## Qu'est-ce que c'est

**Dub Studio** transforme n'importe quelle vidéo en une version doublée dans une autre langue —— **avec le timbre du locuteur cloné, des sous-titres traduits et le texte incrusté localisé à même l'image**. Déposez un clip : une passe automatique intelligente produit le premier jet ; puis un éditeur en direct met **chaque sous-titre, voix, zone de flou, police et titre** sous votre contrôle avec un aperçu instantané.

Par défaut, tout tourne **localement sur votre machine** —— sans cloud ni abonnement : ni vos rushes ni votre voix ne quittent l'ordinateur. Et si votre PC est limité (ne fait pas tourner le Gemma/Higgs local) ou que vous voulez plus de vitesse et de qualité, les parties lourdes (traduction, vision, TTS, transcription) peuvent **en option** être déléguées au cloud via **OpenRouter** —— chaque moteur choisi séparément (local ↔ cloud), avec des voix attribuées automatiquement selon le sexe du locuteur (bêta). La clé est stockée localement ; tout est désactivé par défaut.

C'est **une réécriture entièrement native**. Pas de Python embarqué, pas de torch, pas de wheels CUDA. Tout le pipeline est en **Rust + moteurs natifs C++/CUDA (GGUF/ONNX)** : un processus, démarrage rapide, faible VRAM. Les modèles, moteurs, runtime CUDA/VC++ et ffmpeg sont **téléchargés et installés par l'application elle-même** au premier lancement. **NVIDIA est recommandée mais pas obligatoire** : la séparation dispose d'une build CPU, la diarisation et l'ASR tournent sur CPU, et les étapes lourdes (traduction, vision, TTS) partent dans le cloud, si bien qu'un doublage se monte même sur une machine sans NVIDIA.

## Cinq modes, permutables à la volée

| Mode | Ce qu'il fait |
|------|---------------|
| 🎙️ **Doublage** | Re-doublage complet dans la langue cible avec le **timbre original cloné** —— distribution auto par locuteur ou voix au choix |
| 🗣️ **Voix off** | Voix traduite **par-dessus l'original atténué** —— l'original reste audible en dessous ; équilibre réglable |
| 📝 **Sous-titres** | Sous-titres dans la **langue d'origine**, audio original conservé —— sans doublage ni traduction |
| ✨ **Remix amusant** | Donnez un thème (« comme un pirate », « comme un JT ») → le modèle **réécrit tout le script** et redouble |
| 🎬 **Transcription** | **Transcription diarisée** propre avec disposition par locuteur, lecture façon karaoké, création de voix en un clic, export `.srt`/`.txt` |

Chargez un clip une fois et envoyez-le dans n'importe quel mode depuis l'éditeur.

## Fonctionnalités

- **Clonage de voix** —— le timbre original est cloné et parle la nouvelle langue (moteur natif [Higgs Audio v3](https://huggingface.co/bosonai), GGUF). Distribution auto par locuteur ou votre propre voix depuis un pack.
- **Diarisation des locuteurs** —— qui parle et quand (NVIDIA **Sortformer** v2, jusqu'à 4 voix), une voix distincte par locuteur.
- **Casting des personnages (bêta)** —— un personnage est une paire **« visage + voix »**. L'app rassemble les visages de toute la vidéo, reconnaît la même personne et **la lie à un locuteur par cooccurrence** (celui en gros plan reçoit la voix, un auditeur en arrière-plan non) ; elle choisit automatiquement l'image-avatar la plus nette et **enregistre un profil de casting pour toute la série** —— vous attribuez voix et descriptions une fois, et l'**épisode suivant les applique tout seul**. Un interrupteur **« visages réels / dessin·anime »** adapte la détection des visages au contenu.
- **Choix du moteur ASR** — transcrivez avec **Parakeet-TDT** (GPU, par défaut) ou **Whisper** ([faster-whisper standalone de Purfview](https://github.com/Purfview/whisper-standalone-win), tourne sur CPU) — choisissez la taille du modèle (tiny … large-v3-turbo) et le quant (compute type) directement dans les réglages.
- **Importer des sous-titres prêts** — utilisez votre `.srt`/`.ass` comme transcription exacte : le texte et le timing viennent du fichier au lieu de la reconnaissance auto (les locuteurs sont quand même attribués par diarisation). Cochez **« sous-titres déjà dans la langue cible »** et la traduction est aussi ignorée — une vidéo anglaise + vos sous-titres russes → un doublage russe directement à partir d'eux.
- **Export multilingue** — la **▾** à côté d'Exporter envoie une vidéo dans plusieurs langues d'un coup ; chacune hérite de toutes vos modifications (mise en page des sous-titres, styles, zones de flou, voix clonée) — seuls la traduction et le doublage changent.
- **Sauvegarder et rouvrir des projets** — sauvegarde auto, liste des projets récents sur l'écran d'accueil, et reprenez le travail inachevé en un clic.
- **Recherche dans les listes de voix et de langues** — tapez une partie d'un nom pour filtrer des centaines de voix ou plus de 100 langues ; les langues correspondent aussi par leur nom dans la langue de l'interface.
- **Pipeline composable** — interrupteurs indépendants à l'entrée : audio (original / doublage / voix off / transcription) × sous-titres (aucun / original / traduits) × incrustation dans la vidéo oui/non × remix humoristique. N'importe quelle combinaison — doublage sans sous-titres, sous-titres traduits sans doublage, doublage humoristique avec vos propres voix — aussi en lot et dans l'éditeur.
- **Localisation du texte à l'écran** —— l'OCR détecte le texte incrusté (**PP-OCR** ONNX), **floute l'original** et imprime par-dessus un titre localisé dans un style assorti —— une fonction qu'aucun autre outil n'a.
- **Traduction + analyse visuelle du style** —— la transcription est traduite localement via **Gemma-4 12B** (GGUF, llama.cpp) ; une passe de vision lit la mise en page : style des sous-titres, titres, marques, zones de texte.
- **Séparation vocale SOTA** —— **Mel-Band Roformer** (BSRoformer.cpp natif sur CUDA) sépare la voix de la musique : la piste de fond est **préservée** et le clone s'accroche à une parole propre.
- **26 préréglages de sous-titres** —— karaoké / mot à mot / hormozi / néon et plus, rendus **sur votre image** (WYSIWYG, JASSUB sur le même `.ass` que grave ffmpeg).
- **Transcription karaoké** —— lisez la vidéo et suivez la ligne et le **mot** en cours qui s'illuminent dans la transcription.
- **Éditeur en direct** —— modifiez transcription, voix, style des sous-titres, zones de flou, titres ; **aperçu ~0,17 s/image**, chaque changement visible aussitôt.
- **Re-génération intelligente** —— à l'export, seuls les segments modifiés sont resynthétisés, pas tout le clip.
- **Vos propres répliques** —— insérez des phrases personnalisées dans la transcription ; chacune est doublée avec la voix clonée du locuteur et affichée dans les sous-titres.
- **Traitement par lot** —— file de fichiers, tous avec un même réglage, progression par fichier.
- **Comparaison avant/après** —— original et doublage côte à côte.
- **100+ langues** —— doublage vers toute langue majeure (espagnol, chinois, japonais, arabe, hindi et plus), détection auto de la langue source.
- **Tout format vidéo** —— MP4, MOV, MKV, WEBM, AVI et plus (décodé via ffmpeg).
- **Installation en un bouton + mise à jour auto** —— modèles, moteurs, runtime CUDA/VC++ et ffmpeg se téléchargent au premier lancement ; l'app se met à jour seule.
- **Téléchargements reprenables** —— les gros modèles (10 Go+) reprennent là où ils se sont arrêtés après une coupure, au lieu de tout recommencer.
- **Calculez chaque étape où vous voulez** —— séparation, diarisation et ASR basculent indépendamment entre **GPU, CPU et cloud** ; combinez à votre guise. Les modèles lourds (traduction, vision, TTS) étant délégables à **OpenRouter**, tout le pipeline tourne même sur une machine **sans NVIDIA**.
- **Adaptez à votre matériel** —— chaque moteur propose plusieurs quantifications (TTS Q8/Q6/Q4, traduction Q4…Q8, ASR int8/fp32 ou Whisper tiny…large-v3-turbo, séparation Q8/Q5/Q4) — changez-les dans les réglages ; limitez le lot de prefill et la durée de référence pour les GPU de 8–12 Go et 32 Go de RAM.
- **Entièrement portable** —— rien n'est écrit dans votre profil ; supprimez le dossier, aucune trace.

## Captures

Écran d'accueil —— cinq modes, aperçu de la vidéo choisie, choix de langue, tout format :

![Écran d'accueil de Dub Studio](docs/screenshot-home.png)

Mode transcription —— transcription diarisée avec disposition par locuteur, karaoké et création de voix depuis chaque locuteur en un clic :

![Mode transcription de Dub Studio](docs/screenshot-transcribe.png)

## Prérequis

- **OS :** Windows 10 / 11 (x64)
- **GPU :** NVIDIA avec 8–16 Go de VRAM recommandée —— **ou aucune** : séparation, diarisation et ASR tournent sur CPU, et les modèles lourds partent dans le cloud
- **WebView2** —— préinstallé sur Windows 11 (s'installe seul sur Windows 10)
- **Disque :** ~15 Go pour modèles, moteurs et runtime (téléchargés au premier lancement), plus de la place pour vos projets

Sur une machine NVIDIA, la seule chose à installer à la main est un **[pilote NVIDIA](https://www.nvidia.com/Download/index.aspx)** récent. Tout le reste, l'app le télécharge d'un bouton au premier lancement.

## Démarrage rapide

1. **Téléchargez** la version portable depuis [Releases](https://github.com/timoncool/dub-studio/releases) et décompressez où vous voulez (ou installez via `-setup.exe` / `.msi`).
2. **Lancez** `Dub Studio.exe`.
3. Dans le panneau **premier lancement**, cliquez **Tout télécharger** —— l'app récupère modèles, moteurs et runtime (~15 Go, une fois).
4. **Déposez une vidéo**, choisissez la langue cible → la passe auto produit le premier jet. Réglez tout dans l'éditeur puis cliquez **Exporter**.

> Tout se télécharge et vit **dans le dossier de l'app**. Modèles, caches et projets ne vont nulle part ailleurs.

## Comment ça marche

`analyze()` est une première passe fixe : séparation → ASR avec timing par mot → diarisation → traduction contextuelle + vision (style des sous-titres / titres / marques) → OCR (mise en page / zones de flou). Le résultat est un document **Project** éditable. Chaque modification est un patch dessus avec aperçu ~0,17 s/image ; l'export ne rejoue que **les étapes salies**.

**Stack :** une coque native **Tauri 2 (Rust)** lance `dub-server` (axum) sur un port local et ouvre une fenêtre sur la SPA —— React 19 + Vite + Tailwind + react-konva sur JASSUB. Moteurs : Parakeet-TDT ou Whisper (ASR) · Sortformer (diarisation) · Gemma-4-12B GGUF (traduction + vision, llama.cpp) · Higgs Audio v3 (TTS) · Mel-Band Roformer (séparation, BSRoformer.cpp) · PP-OCR (ONNX) · ffmpeg/NVENC. **Aucun processus Python à l'exécution.**

### Compiler depuis les sources

```bash
git clone https://github.com/timoncool/dub-studio.git
cd dub-studio

cd frontend && npm install && npm run build && cd ..   # 1) SPA
cargo build --release -p dub-server                     # 2) serveur natif (axum)
cd desktop && npm install && npx tauri build            # 3) coque bureau (Tauri)
```

Nécessite Node 20+, Rust (toolchain MSVC) et WebView2. Les moteurs natifs n'ont pas besoin d'être recompilés —— l'app télécharge des binaires précompilés.

## Contributions et forks

**Les collaborateurs sont les bienvenus.** Je serais vraiment ravi de voir Dub Studio porté sur d'autres plateformes et GPU — l'architecture le permet, je n'ai simplement pas le temps de faire les portages moi-même. Si vous le voulez sur **GPU AMD / Intel, macOS ou Linux**, forkez-le — les PR sont les bienvenues.

**Les localisations supplémentaires** sont tout aussi bienvenues : l'app et la landing existent en 6 langues aujourd'hui — traduisez les fichiers de langue (`frontend/src/locales/` et le dictionnaire dans `docs/index.html`) et ouvrez une PR pour ajouter la vôtre.

## Auteurs

- **Nerual Dreming** —— [Telegram](https://t.me/nerual_dreming) | [neuro-cartel.com](https://neuro-cartel.com) | fondateur d'[ArtGeneration.me](https://artgeneration.me)
- **Neuro-Soft** —— [Telegram](https://t.me/neuroport) | applis IA portables

## Crédits

- **[Boson AI](https://huggingface.co/bosonai)** —— le modèle Higgs Audio v3 ; **[drbaph / Higgs-Audio-v3-Studio](https://huggingface.co/drbaph/Higgs-Audio-v3-Studio)** —— quantifications GGUF et `audiocpp_engine.dll` natif.
- **[NVIDIA Parakeet](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)** et **[Sortformer](https://huggingface.co/nvidia)** —— ASR et diarisation ; poids ONNX de [istupakov/parakeet-tdt-0.6b-v3-onnx](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) et [altunenes/parakeet-rs](https://github.com/altunenes/parakeet-rs).
- **[Google Gemma](https://huggingface.co/google/gemma-4-12b-it-qat-q4_0-gguf)** —— Gemma-4 12B (traduction + vision), via [llama.cpp](https://github.com/ggml-org/llama.cpp).
- **[chenmozhijin / BSRoformer.cpp](https://github.com/chenmozhijin/BSRoformer.cpp)** et **[GaboxR67](https://huggingface.co/GaboxR67)** —— le moteur natif et le modèle Mel-Band Roformer.

## Soutenir l'auteur

Je crée des logiciels open source et je fais de la recherche en IA —— l'essentiel est en accès libre. Les dons me permettent de créer et chercher davantage.

**[Toutes les façons de soutenir](DONATE.md)** | **[dalink.to/nerual_dreming](https://dalink.to/nerual_dreming)** | **[boosty.to/neuro_art](https://boosty.to/neuro_art)**

- **BTC :** `1E7dHL22RpyhJGVpcvKdbyZgksSYkYeEBC`
- **ETH (ERC20) :** `0xb5db65adf478983186d4897ba92fe2c25c594a0c`
- **USDT (TRC20) :** `TQST9Lp2TjK6FiVkn4fwfGUee7NmkxEE7C`

## Licence

Le code de l'app est [MIT](LICENSE). Les poids des modèles conservent leurs licences (Higgs Audio v3 —— Boson AI recherche/non commercial ; Gemma —— Gemma Terms ; etc.) —— audités avant chaque publication.
